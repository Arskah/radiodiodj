//! Shared playback primitives: the deck command vocabulary, the event topic
//! names, and the read/decode helpers every deck uses.
//!
//! The decks themselves live in [`super::deck`], driven by a worker that serves
//! a whole [`super::bus::ProgramBus`] (or the lone [`super::cue::CueDeck`]).

use anyhow::{Context, Result};
use rodio::source::SkipDuration;
use rodio::{Decoder, Sink, Source};
use std::fs::File;
use std::io::{Cursor, ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::cue_points::{CuePoints, Resolved};
use super::envelope::Enveloped;

/// A background read that has delivered no further bytes within this budget is
/// treated as a wedged (e.g. networked) mount. The audio worker stops waiting
/// on it and declares a timeout; the detached read thread is abandoned (a
/// blocked `read()` cannot be cancelled — it unwinds whenever the OS finally
/// errors the mount).
///
/// The budget bounds a *stall*, never the read, which is why the read reports
/// progress at all ([`read_file_watched`]). See `docs/audio.md`, *Whole-file
/// reads*.
pub(super) const READ_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(10);

/// How much of a file one `read` call is asked for while filling RAM.
///
/// A count published per chunk is a throughput floor as well as a stall
/// detector: a share too slow to deliver one chunk inside the watchdog budget
/// still looks stalled. 64 KiB against the 10 s default puts that floor around
/// 6.5 KB/s — a share slower than that is not one a show can run off — while
/// keeping the syscall count for a 40 MB track in the hundreds.
const READ_CHUNK: usize = 64 * 1024;

/// Backoff delays applied between failed read attempts. The read thread makes
/// one initial attempt plus one retry per entry (4 attempts, 3 backoffs) before
/// giving up. Retries cover *transient* failures (`Err`); hangs are handled by
/// the watchdog, not retry.
const READ_RETRY_BACKOFFS: [Duration; 3] = [
    Duration::from_millis(500),
    Duration::from_millis(1000),
    Duration::from_millis(2000),
];

/// While a load is deferred because no audio device could be opened, the worker
/// retries opening the output on this cadence so playback self-heals without any
/// user action (a stalled/absent device at launch — see issue #259).
pub(super) const OPEN_RETRY_INTERVAL: Duration = Duration::from_secs(2);

/// Network-resilience timeouts for the player worker, supplied at spawn from the
/// stored config. Captured for the worker's lifetime. Clamped on write (see
/// `persist::config::set_tuning`), so `read_retry_backoffs` is always non-empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerTuning {
    /// See [`READ_WATCHDOG_TIMEOUT`]. A budget with no progress, not a budget
    /// for the whole read.
    pub read_watchdog_timeout: Duration,
    /// See [`OPEN_RETRY_INTERVAL`].
    pub open_retry_interval: Duration,
    /// See [`READ_RETRY_BACKOFFS`]. Must be non-empty (enforced on write).
    pub read_retry_backoffs: Vec<Duration>,
}

impl Default for PlayerTuning {
    fn default() -> Self {
        Self {
            read_watchdog_timeout: READ_WATCHDOG_TIMEOUT,
            open_retry_interval: OPEN_RETRY_INTERVAL,
            read_retry_backoffs: READ_RETRY_BACKOFFS.to_vec(),
        }
    }
}

/// Whole track file resident in RAM. Shared (cheaply cloned) between the
/// playing `Decoder` and the retained copy used for seeking, so playback and
/// seek never touch the (possibly networked) filesystem again after load.
pub(super) type Bytes = Arc<[u8]>;

pub enum Cmd {
    Load {
        /// DB track id. Used to consult the shared prefetch cache before
        /// falling back to a filesystem read, and to name the track in the
        /// `:load-failed` event when a load fails or times out.
        id: i64,
        path: PathBuf,
        duration: Option<f64>,
        /// The markers to apply to this airing, already chosen: the radio edit
        /// off the track row, or an item override. The worker applies what it
        /// is given and never consults the library itself.
        cue_points: CuePoints,
        /// Where to start, in **air** seconds — measured from `cue_in`, like
        /// everything else crossing the Tauri boundary. Applied once the bytes
        /// are decoded: a `Seek` issued straight after a `Load` would find no
        /// bytes to seek in and be dropped, so resuming a position has to
        /// travel with the load.
        start_at: f64,
        /// Start playing once ready. `false` loads the track parked and silent,
        /// which is what restoring a session needs: a restart must not put
        /// audio on air by itself.
        autoplay: bool,
        /// Linear factor levelling this track to the ReplayGain reference,
        /// already resolved against the setting and the track's measurement.
        /// `1.0` leaves the track as mastered — the setting is off, or nothing
        /// has been measured for it yet.
        ///
        /// Resolved by the caller for the same reason `cue_points` is: the
        /// worker applies what it is given and never consults the library.
        gain: f32,
    },
    Play,
    Pause,
    Stop,
    /// Air seconds, measured from `cue_in`. The worker adds the offset.
    Seek(f64),
    SetVolume(f32),
    /// Ramp this deck's gain to `to` over `ms`, then run `on_complete`.
    ///
    /// Runtime only, never persisted, and never a replacement for the
    /// operator's deck volume: the ramp multiplies it (see `Deck::gain`). Any
    /// other transport command cancels the ramp and restores full gain, so the
    /// next track on this deck always starts where the operator left the fader.
    Fade {
        to: f32,
        ms: u64,
        on_complete: Option<RampDone>,
    },
    /// Move the `main` role to the armed deck now, and fade the outgoing track
    /// out underneath the incoming one over `fade_ms`.
    ///
    /// Aimed at `main`, but it is the one command that acts on two decks, so
    /// the worker intercepts it in its dispatch loop rather than applying it to
    /// a single deck. A no-op unless a deck is armed and decoded and no tail is
    /// already playing — the caller falls back to a plain fade-out plus a next.
    HandOverNow {
        fade_ms: u64,
    },
}

/// What a completed ramp does to the deck it ran on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RampDone {
    /// Stop the deck: what a fade to silence is for. The deck is reset exactly
    /// as [`Cmd::Stop`] resets it, so nothing downstream needs a second
    /// termination rule.
    Stop,
    /// Stop the deck *and* announce the track as ended, so whatever follows an
    /// ordinary end-of-track follows this too.
    ///
    /// This is how *Fade to next* degrades when nothing is armed to overlap
    /// with: the playlist advances because the track ended, under the rules it
    /// already has — it advances in Auto and stops in Manual, and a transport
    /// command mid-ramp cancels the fade and with it the ending.
    EndTrack,
}

/// The event topics one deck emits on. Built from a prefix, which is a *role*
/// (`main-deck`, `arm-deck`) for decks on the program bus — so the renderer,
/// the broadcast service, and the now-playing webhook keep addressing `main`
/// no matter which physical deck is on air.
pub(super) struct Topics {
    pub time: String,
    pub duration: String,
    pub pause_state: String,
    pub ended: String,
    pub error: String,
    pub buffering: String,
    pub loaded: String,
    pub load_failed: String,
    pub output_unavailable: String,
}

impl Topics {
    pub(super) fn new(prefix: &str) -> Self {
        Self {
            time: format!("{prefix}:time"),
            duration: format!("{prefix}:duration"),
            pause_state: format!("{prefix}:pause-state"),
            ended: format!("{prefix}:ended"),
            error: format!("{prefix}:error"),
            buffering: format!("{prefix}:buffering"),
            loaded: format!("{prefix}:loaded"),
            load_failed: format!("{prefix}:load-failed"),
            output_unavailable: format!("{prefix}:output-unavailable"),
        }
    }
}

/// How far short of the target the container seek aims before the remainder is
/// decoded. Symphonia estimates an MP3 seek by bitrate when the file carries no
/// Xing TOC, which is fine for scrubbing but not for a stored marker that must
/// sound identical on every airing. Decoding the last fragment makes the landing
/// sample-exact for the cost of this much pre-roll.
const SEEK_PREROLL: Duration = Duration::from_millis(200);

/// Hand the sink the track as `cue` shapes it, starting from the absolute file
/// position `start`: seek in, level to `gain`, apply the fade envelope, stop at
/// the out-point.
///
/// Running the sink dry at the out-point is what ends a trimmed track: the
/// existing `sink.empty()` → `:ended` path fires naturally, with no second
/// termination rule to keep in step.
///
/// `gain` is applied here, at the source, rather than through
/// `sink.set_volume()`, for the reason `envelope` gives for the fades: sink
/// gain belongs to the live fade-out and the segue ramps, and a second writer
/// on it would fight them. It also has to land before the bus mixer sums the
/// two decks, or a handover between tracks mastered at different levels
/// crossfades wrong — and nothing downstream of the mixer can unpick that.
/// This one function serves both program decks and the cue deck, so the
/// headphone feed is levelled by the same code.
pub(super) fn append_span(
    sink: &Sink,
    source: Decoder<Cursor<Bytes>>,
    start: Duration,
    cue: &Resolved,
    gain: f32,
) {
    let take = cue
        .take_from(start.as_secs_f64())
        .map(Duration::from_secs_f64);
    let source = seek_to(source, start);
    // A track with neither ramps nor levelling is handed to the sink unwrapped
    // rather than multiplied by 1.0 for its whole length.
    match (cue.has_fades(), gain != 1.0) {
        (true, true) => append_take(
            sink,
            Enveloped::new(source, start.as_secs_f64(), *cue).amplify(gain),
            take,
        ),
        (true, false) => append_take(
            sink,
            Enveloped::new(source, start.as_secs_f64(), *cue),
            take,
        ),
        (false, true) => append_take(sink, source.amplify(gain), take),
        (false, false) => append_take(sink, source, take),
    }
}

/// Seek `source` to `start` in two stages: the container's own seek (a binary
/// search over the index) to just short of the target, then decoding forward
/// for the remainder. A failing container seek decodes forward from zero, which
/// is what the code did before cue points existed.
fn seek_to(
    mut source: Decoder<Cursor<Bytes>>,
    start: Duration,
) -> SkipDuration<Decoder<Cursor<Bytes>>> {
    if start.is_zero() {
        return source.skip_duration(Duration::ZERO);
    }
    let coarse = start.saturating_sub(SEEK_PREROLL);
    if coarse.is_zero() {
        return source.skip_duration(start);
    }
    match source.try_seek(coarse) {
        Ok(()) => source.skip_duration(start - coarse),
        Err(e) => {
            log::warn!(
                "player: container seek failed ({}); skip_duration fallback",
                e
            );
            source.skip_duration(start)
        }
    }
}

fn append_take<S>(sink: &Sink, source: S, take: Option<Duration>)
where
    S: Source + Send + 'static,
{
    match take {
        Some(d) => sink.append(source.take_duration(d)),
        None => sink.append(source),
    }
}

/// Keep a restored position inside the track, in air seconds against the air
/// duration. A session saved before the file changed — or a cue-out that has
/// since moved earlier — would otherwise seek past the end: `take_duration`
/// would yield nothing, `sink.empty()` would fire at once, and auto-advance
/// would silently skip the resumed track at launch. Restarting it is the
/// recoverable answer. A track whose duration is unknown is trusted as-is.
pub(super) fn clamp_start(seconds: f64, duration: Option<f64>) -> f64 {
    let start = seconds.max(0.0);
    match duration {
        Some(d) if d > 0.0 && start >= d => 0.0,
        _ => start,
    }
}

/// Read a file with bounded retry + backoff for *transient* failures. Makes an
/// initial attempt plus one retry per `READ_RETRY_BACKOFFS` entry, sleeping the
/// matching backoff between attempts, and returns the first success or the last
/// error. `read`/`sleep` are injected so tests exercise the schedule without
/// touching the filesystem or actually sleeping.
pub(super) fn read_with_retry<R, S>(
    mut read: R,
    mut sleep: S,
    backoffs: &[Duration],
) -> Result<Bytes>
where
    R: FnMut() -> Result<Bytes>,
    S: FnMut(Duration),
{
    let mut last_err: Option<anyhow::Error> = None;
    // Attempt indices 0..=len: index 0 is the initial try, and after a failing
    // attempt `i` we back off by `backoffs[i]` if one exists.
    for attempt in 0..=backoffs.len() {
        match read() {
            Ok(bytes) => return Ok(bytes),
            Err(e) => {
                last_err = Some(e);
                if let Some(delay) = backoffs.get(attempt) {
                    sleep(*delay);
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("read failed with no attempts")))
}

/// Bytes a read has delivered so far, shared with whoever is watching it.
///
/// Monotonic and never reset, so a [`read_with_retry`] attempt that starts the
/// file over still reads as progress rather than as a rewind — what the
/// watchdog asks is whether the share is sending anything at all.
pub(super) type Progress = Arc<AtomicU64>;

/// A [`Progress`] at zero, for a read about to start. Whoever else watches it
/// — the deck that issued the read, or a deck mirroring this one's count
/// through the cache's in-flight claims — takes a clone of the handle.
pub(super) fn fresh_progress() -> Progress {
    Arc::new(AtomicU64::new(0))
}

/// The last thing a watcher saw of a read: how many bytes had been delivered,
/// and when that count last changed.
///
/// Pure, so both watchers — the deck's watchdog and a deck waiting on another
/// reader's copy in [`super::cache`] — decide "stalled" the same way, on an
/// injected clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Seen {
    /// Bytes the read had delivered when it was last looked at.
    pub bytes: u64,
    /// When `bytes` last changed — the instant a stall is measured from.
    pub at: Instant,
}

impl Seen {
    /// A read that has just been issued: nothing delivered yet, and the clock
    /// on the first chunk starts now.
    pub(super) fn issued(now: Instant) -> Self {
        Self { bytes: 0, at: now }
    }

    /// Fold in a fresh byte count. The stamp moves only when the count does, so
    /// the age of a `Seen` is the length of the current stall and nothing else.
    pub(super) fn observe(self, bytes: u64, now: Instant) -> Self {
        if bytes == self.bytes {
            self
        } else {
            Self { bytes, at: now }
        }
    }

    /// Whether nothing has arrived for `budget`.
    pub(super) fn stalled(self, now: Instant, budget: Duration) -> bool {
        now.saturating_duration_since(self.at) >= budget
    }
}

/// Read a whole file into RAM, publishing the running byte count into
/// `progress` as chunks land.
///
/// The count is what makes a slow share distinguishable from a dead one: the
/// watchdog fires on a count that stops moving, so a read is never failed for
/// the size of the file. Chunking is only for the reporting — the deck still
/// decodes from one contiguous buffer.
pub(super) fn read_file_watched(path: &Path, progress: &Progress) -> Result<Bytes> {
    let ctx = || format!("read {}", path.display());
    let mut file = File::open(path).with_context(ctx)?;
    let hint = file
        .metadata()
        .map(|m| usize::try_from(m.len()).unwrap_or(0))
        .unwrap_or(0);
    // Read into the destination, never through a staging chunk: a whole-file
    // copy per track load is a real cost, and the chunking is only here so the
    // count gets published on the way.
    let mut buf: Vec<u8> = vec![0u8; hint.max(READ_CHUNK)];
    let mut filled = 0usize;
    loop {
        if filled == buf.len() {
            // The file grew past its metadata, or there was none.
            buf.resize(filled + READ_CHUNK, 0);
        }
        let end = (filled + READ_CHUNK).min(buf.len());
        match file.read(&mut buf[filled..end]) {
            Ok(0) => break,
            Ok(read) => {
                filled += read;
                progress.fetch_add(read as u64, Ordering::Relaxed);
            }
            // `read_to_end` retries this one, and `fs::read` used to get that
            // for free; a signal is not a failed read.
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(e).with_context(ctx),
        }
    }
    buf.truncate(filled);
    Ok(Arc::from(buf.into_boxed_slice()))
}

pub(super) fn decode_bytes(bytes: Bytes) -> Result<(Decoder<Cursor<Bytes>>, Option<f64>)> {
    let decoder = Decoder::new(Cursor::new(bytes)).context("decoder")?;
    let total = decoder.total_duration().map(|d| d.as_secs_f64());
    Ok((decoder, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid 16-bit mono PCM WAV in memory so decode tests are
    /// hermetic (no fixture files, no audio device).
    fn synth_wav(sample_rate: u32, samples: u32) -> Vec<u8> {
        let bits_per_sample = 16u16;
        let channels = 1u16;
        let byte_rate = sample_rate * channels as u32 * (bits_per_sample as u32 / 8);
        let block_align = channels * (bits_per_sample / 8);
        let data_len = samples * (bits_per_sample as u32 / 8);
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data_len).to_le_bytes());
        w.extend_from_slice(b"WAVE");
        w.extend_from_slice(b"fmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes()); // PCM
        w.extend_from_slice(&channels.to_le_bytes());
        w.extend_from_slice(&sample_rate.to_le_bytes());
        w.extend_from_slice(&byte_rate.to_le_bytes());
        w.extend_from_slice(&block_align.to_le_bytes());
        w.extend_from_slice(&bits_per_sample.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..samples {
            let v = ((i % 32) as i16) * 100;
            w.extend_from_slice(&v.to_le_bytes());
        }
        w
    }

    #[test]
    fn decode_bytes_reports_duration() {
        let sample_rate = 8000;
        let samples = 8000; // exactly 1 second
        let bytes: Bytes = Arc::from(synth_wav(sample_rate, samples).into_boxed_slice());
        let (_source, duration) = decode_bytes(bytes).expect("decode");
        let d = duration.expect("duration present");
        assert!((d - 1.0).abs() < 0.05, "expected ~1.0s, got {d}");
    }

    #[test]
    fn decode_bytes_rejects_garbage() {
        let bytes: Bytes = Arc::from(vec![0u8, 1, 2, 3, 4, 5].into_boxed_slice());
        assert!(decode_bytes(bytes).is_err());
    }

    #[test]
    fn read_file_missing_path_errors() {
        assert!(read_file_watched(
            Path::new("/nonexistent/radiodiodj/nope.wav"),
            &fresh_progress()
        )
        .is_err());
    }

    /// A file spanning several chunks comes back whole, and the published count
    /// ends at its length — which is what the watchdog watches.
    #[test]
    fn a_watched_read_returns_the_whole_file_and_counts_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("track.bin");
        let written: Vec<u8> = (0..READ_CHUNK * 2 + 7).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &written).expect("write");

        let progress = fresh_progress();
        let bytes = read_file_watched(&path, &progress).expect("read");
        assert_eq!(&bytes[..], &written[..]);
        assert_eq!(progress.load(Ordering::Relaxed), written.len() as u64);

        // The buffer is sized from the metadata and truncated to what arrived,
        // so a file shorter than one chunk — or empty — is not padded.
        let short = dir.path().join("short.bin");
        std::fs::write(&short, [1u8, 2, 3]).expect("write");
        assert_eq!(
            &read_file_watched(&short, &fresh_progress()).expect("read")[..],
            &[1u8, 2, 3]
        );
        let empty = dir.path().join("empty.bin");
        std::fs::write(&empty, []).expect("write");
        assert!(read_file_watched(&empty, &fresh_progress())
            .expect("read")
            .is_empty());
    }

    /// A retried read starts the file over, and the count must not: the
    /// watchdog asks whether the share is sending anything, so a fresh attempt
    /// is progress, never a rewind.
    #[test]
    fn a_retried_read_never_rewinds_the_count() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("track.bin");
        std::fs::write(&path, vec![7u8; READ_CHUNK]).expect("write");

        let progress = fresh_progress();
        let mut attempts = 0u32;
        let result = read_with_retry(
            || {
                attempts += 1;
                let bytes = read_file_watched(&path, &progress)?;
                if attempts == 1 {
                    anyhow::bail!("share hiccuped after the bytes landed")
                }
                Ok(bytes)
            },
            |_| {},
            &READ_RETRY_BACKOFFS,
        );
        assert!(result.is_ok());
        assert_eq!(
            progress.load(Ordering::Relaxed),
            (READ_CHUNK * 2) as u64,
            "both attempts counted, so the count only ever climbs"
        );
    }

    /// The end-to-end of the pair: a file that trickles in publishes its count
    /// *during* the read, so a watcher applying the watchdog rule never calls
    /// it a stall. A FIFO stands in for the slow share — the only way to have
    /// bytes arrive over time without one.
    #[cfg(unix)]
    #[test]
    fn a_trickling_read_publishes_progress_while_it_runs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let fifo = dir.path().join("slow.bin");
        assert!(std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo")
            .success());

        let writer_path = fifo.clone();
        let writer = std::thread::spawn(move || {
            use std::io::Write;
            let mut f = std::fs::File::create(&writer_path).expect("open fifo for writing");
            for _ in 0..8 {
                f.write_all(&vec![9u8; 64 * 1024]).expect("write chunk");
                f.flush().expect("flush");
                std::thread::sleep(Duration::from_millis(20));
            }
        });

        let progress = fresh_progress();
        let watched = Arc::clone(&progress);
        let reader_path = fifo.clone();
        let reader = std::thread::spawn(move || read_file_watched(&reader_path, &watched));

        // Watch the way the worker loop does, on a budget far shorter than the
        // whole read takes.
        let budget = Duration::from_millis(500);
        let mut seen = Seen::issued(Instant::now());
        let mut samples = 0u32;
        while !reader.is_finished() {
            std::thread::sleep(Duration::from_millis(5));
            let now = Instant::now();
            seen = seen.observe(progress.load(Ordering::Relaxed), now);
            assert!(
                !seen.stalled(now, budget),
                "a read that keeps delivering is not a stall"
            );
            samples += 1;
        }

        let bytes = reader.join().expect("reader thread").expect("read");
        writer.join().expect("writer thread");
        assert_eq!(bytes.len(), 8 * 64 * 1024);
        assert!(samples > 1, "the read was observed while it was running");
    }

    #[test]
    fn a_stall_is_measured_from_the_last_byte_not_the_first() {
        let issued = Instant::now();
        let seen = Seen::issued(issued);
        let later = issued.checked_add(Duration::from_secs(30)).unwrap();

        // A count that moved resets the stall clock.
        let moved = seen.observe(4096, later);
        assert_eq!(moved.at, later);
        assert!(!moved.stalled(later, Duration::from_secs(10)));

        // A count that did not move keeps the earlier stamp, so the stall goes
        // on being measured from it.
        let same = moved.observe(4096, later.checked_add(Duration::from_secs(5)).unwrap());
        assert_eq!(same.at, later);
        assert!(same.stalled(
            later.checked_add(Duration::from_secs(10)).unwrap(),
            Duration::from_secs(10)
        ));
    }

    #[test]
    fn a_restored_position_survives_when_it_is_inside_the_track() {
        assert_eq!(clamp_start(12.5, Some(200.0)), 12.5);
        assert_eq!(clamp_start(0.0, Some(200.0)), 0.0);
    }

    /// A position past the end would seek out of range and the track would be
    /// silently skipped at launch. Restarting it is the recoverable answer.
    #[test]
    fn a_restored_position_past_the_end_restarts_the_track() {
        assert_eq!(clamp_start(500.0, Some(200.0)), 0.0);
        assert_eq!(clamp_start(200.0, Some(200.0)), 0.0);
    }

    #[test]
    fn a_negative_restored_position_clamps_to_the_start() {
        assert_eq!(clamp_start(-4.0, Some(200.0)), 0.0);
        assert_eq!(clamp_start(-4.0, None), 0.0);
    }

    /// The restored position is in air seconds, so it is clamped against air
    /// time: a cue-out moved in behind the operator's back restarts the track
    /// instead of landing past the out-point and being skipped.
    #[test]
    fn a_restored_position_is_clamped_against_air_time_not_file_time() {
        // A 200s file trimmed to 20s of air: 30s was valid last session.
        let air = CuePoints {
            cue_in_ms: Some(10_000),
            cue_out_ms: Some(30_000),
            ..Default::default()
        }
        .resolve(Some(200.0))
        .air_duration();
        assert_eq!(air, Some(20.0));
        assert_eq!(clamp_start(30.0, air), 0.0);
        assert_eq!(clamp_start(12.0, air), 12.0);
    }

    /// An unknown duration (no tag, not yet decoded) is no reason to throw the
    /// position away.
    #[test]
    fn a_restored_position_is_trusted_when_the_duration_is_unknown() {
        assert_eq!(clamp_start(12.5, None), 12.5);
        assert_eq!(clamp_start(12.5, Some(0.0)), 12.5);
    }

    /// The backoff schedule is the agreed 0.5s / 1s / 2s with three entries
    /// (three retries after the initial attempt).
    #[test]
    fn backoff_schedule_is_half_one_two_seconds() {
        assert_eq!(
            READ_RETRY_BACKOFFS,
            [
                Duration::from_millis(500),
                Duration::from_millis(1000),
                Duration::from_millis(2000),
            ]
        );
    }

    /// A transient failure that clears within the retry budget eventually
    /// succeeds, and the recorded backoffs follow the schedule exactly.
    #[test]
    fn read_with_retry_succeeds_after_transient_failures() {
        let mut attempts = 0u32;
        let mut slept: Vec<Duration> = Vec::new();
        let result = read_with_retry(
            || {
                attempts += 1;
                if attempts <= 2 {
                    Err(anyhow::anyhow!("transient"))
                } else {
                    Ok(Arc::from(vec![1u8, 2, 3].into_boxed_slice()))
                }
            },
            |d| slept.push(d),
            &READ_RETRY_BACKOFFS,
        );
        assert!(result.is_ok());
        assert_eq!(attempts, 3, "initial attempt + 2 retries");
        // Backoffs applied before retry 1 and retry 2 only.
        assert_eq!(
            slept,
            vec![Duration::from_millis(500), Duration::from_millis(1000)]
        );
    }

    /// An always-failing read exhausts the budget: 4 attempts (initial + 3
    /// retries), sleeping the full 0.5s / 1s / 2s schedule, then returns Err.
    #[test]
    fn read_with_retry_gives_up_after_exhausting_backoffs() {
        let mut attempts = 0u32;
        let mut slept: Vec<Duration> = Vec::new();
        let result = read_with_retry(
            || {
                attempts += 1;
                Err::<Bytes, _>(anyhow::anyhow!("always fails"))
            },
            |d| slept.push(d),
            &READ_RETRY_BACKOFFS,
        );
        assert!(result.is_err());
        assert_eq!(attempts, READ_RETRY_BACKOFFS.len() as u32 + 1);
        assert_eq!(slept, READ_RETRY_BACKOFFS.to_vec());
    }

    /// `PlayerTuning::default` equals the module constants, so an untouched
    /// config runs with these exact timeouts.
    #[test]
    fn player_tuning_default_matches_constants() {
        let t = PlayerTuning::default();
        assert_eq!(t.read_watchdog_timeout, READ_WATCHDOG_TIMEOUT);
        assert_eq!(t.open_retry_interval, OPEN_RETRY_INTERVAL);
        assert_eq!(t.read_retry_backoffs, READ_RETRY_BACKOFFS.to_vec());
    }

    /// The supplied backoff slice drives the retry count: one attempt per entry
    /// plus the initial try.
    #[test]
    fn read_with_retry_honours_custom_backoffs() {
        let mut attempts = 0u32;
        let mut slept: Vec<Duration> = Vec::new();
        let backoffs = [Duration::from_millis(10)];
        let result = read_with_retry(
            || {
                attempts += 1;
                Err::<Bytes, _>(anyhow::anyhow!("always fails"))
            },
            |d| slept.push(d),
            &backoffs,
        );
        assert!(result.is_err());
        assert_eq!(attempts, 2, "initial attempt + 1 retry");
        assert_eq!(slept, vec![Duration::from_millis(10)]);
    }

    /// Topics are built from the role prefix, which is what keeps every existing
    /// `main-deck:*` listener working once roles start moving between decks.
    #[test]
    fn topics_carry_the_role_prefix() {
        let t = Topics::new("main-deck");
        assert_eq!(t.time, "main-deck:time");
        assert_eq!(t.ended, "main-deck:ended");
        assert_eq!(t.loaded, "main-deck:loaded");
        assert_eq!(t.load_failed, "main-deck:load-failed");
        assert_eq!(t.output_unavailable, "main-deck:output-unavailable");
    }
}
