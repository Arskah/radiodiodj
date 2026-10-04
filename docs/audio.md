# Audio — decode, output devices and levels

The audio path from a file on disk to a sample on an output device: how a track
is read and decoded, which device it lands on, how loud it plays, and what
"duration" means once cue points apply.

The on-air topology — the mixer, the deck roles, handover and the live fade
buttons — is [program-bus.md](./program-bus.md). The markers this module
consumes are [cue-points.md](./cue-points.md). This page covers everything
underneath both.

| topic                              | detail                                         |
| ---------------------------------- | ---------------------------------------------- |
| mixer, roles, handover, live fades | [program-bus.md](./program-bus.md)             |
| the five markers and their editor  | [cue-points.md](./cue-points.md)               |
| deriving markers from a decode     | [cue-auto-analysis.md](./cue-auto-analysis.md) |
| where the modules sit              | [architecture.md](./architecture.md)           |

## The decode path

Playback is in-process Rust: [rodio](https://docs.rs/rodio) driving
[symphonia](https://docs.rs/symphonia), on worker threads inside the Tauri
backend. There is no browser `<audio>` element, no `media://` protocol, no
transcoder and no external binary to bundle or sign — the Chromium audio
pipeline was replaced wholesale in the Tauri port (#76), and the earlier plan to
shell out to mpv was dropped with it.

Supported extensions are the table in `audio_measure/formats.rs`:

```
mp3  flac  wav  ogg  oga  aac  m4a  opus  webm  aiff  aif  mka  mp2
```

A scan indexes these and skips everything else. WMA is deliberately absent —
see [library.md](./library.md#unsupported-formats) for converting a folder of
it.

`audio/player.rs` holds what every deck shares: the `Cmd` vocabulary a deck
worker accepts, the `Topics` table naming its events, and the read/decode
helpers. `audio/deck.rs` is one deck — a rodio `Sink` plus its load, seek and
end-of-track bookkeeping — and the worker loop that ticks a whole set of decks
against one output.

### Whole-file reads

A deck does not stream from the filesystem. It reads the **whole file into RAM**
and decodes from a `Cursor` over those bytes. On a local disk this is merely
tidy; on the SMB or NFS share a station actually keeps its music on it is the
difference between a stall and a broadcast, because a share that wedges mid-file
cannot stall a decoder that is no longer reading from it.

The read happens on its own thread, with two protections in `audio/player.rs`:

- **Retry.** Four attempts with backoff between them, covering transient errors.
- **A watchdog over the stall, not the read.** The read is taken in chunks and
  publishes a running byte count (`read_file_watched`); the worker loop notes it
  on every tick, and `READ_WATCHDOG_TIMEOUT` (10 s) bounds how long that count
  may stand still. A `read()` blocked on a dead mount cannot be cancelled, so
  the thread is abandoned rather than joined; it unwinds whenever the OS finally
  errors the mount.

  **The budget is never the read's.** A 40 MB FLAC on a share that manages a
  megabyte a second takes four times the budget to arrive, and a share that is
  merely slow is a share that works — timing the whole read dropped exactly
  those tracks off air (#504). A dead mount still fails in the same 10 s,
  because a read that has delivered nothing is what the budget describes.

  The count moves a chunk at a time (`READ_CHUNK`, 64 KiB), so the budget is
  also a throughput floor: a share that cannot deliver one chunk inside it
  still reads as stalled. At the defaults that is about 6.5 KB/s — two orders
  of magnitude below the rate that used to be required, and below any share a
  show can run off.

- **A dead-air limit.** `DEAD_AIR_LIMIT` (3 s, `player.deadAirLimitMs`) is the
  second bound, and the reason the watchdog can afford to be patient. A read
  that keeps arriving is not a wedged mount, but air that keeps being silent is
  still a show with nothing on it, so a load is given up on once it has kept
  the station quiet for this long — whatever the read is doing.

  The two bounds do not stack, they divide the loads between them. The watchdog
  asks whether a read will ever finish, which is a question about every load on
  every deck; the dead-air limit asks whether the playlist should put something
  else on, which is only a question on a silent on-air deck. So the bound that
  fires is the watchdog for an arm preload, a parked restore, a cue audition or
  an operator's own choice of track — on the cue deck it is the only bound there
  will ever be, since nothing else watches a deck that is off the program bus —
  and the dead-air limit for a load the playlist issued, which reaches 3 s long
  before the watchdog's 10 s could.

  It applies only where there is air to lose _and_ something to put on
  instead: the deck holding `main` on the program bus, nothing audible anywhere
  in the set, and a load the **playlist** issued and asked to play. A track
  loading ahead of a handover has the rest of the outgoing track to arrive in,
  a session restore is parked silent on purpose, and the cue deck is
  monitoring — none of them are bounded by it. `DeckSet::on_air` is what the cue
  worker says no with.

  Nor is a track an operator put on air by hand, nor anything at all while
  auto-advance is off (`Cmd::Load`'s `bound_dead_air`, set from `Effect::Play`
  as `playlist_issued && auto_advance`). Both come off the same premise:
  **giving up is only a recovery if something else goes on instead.** The
  engine has a queue and knows which of it is resident — but it acts on
  `{role}:load-failed` only while it is the one advancing (`on_load_failed`
  returns immediately otherwise), and giving up on the track someone chose is
  just a different track, which is not theirs to choose. They can skip it themselves in less time than any limit
  would allow, and in practice this is the load that reaches the share at all:
  playlist tracks are prefetched, so a miss on air is usually a library track
  started by hand. The stall watchdog still covers it, so a dead mount fails the
  same way it always did.

  Both bounds end the same way, in `abandon_load`: `{role}:load-failed`, which
  the playlist turns into skip-to-cached. What the operator is told follows what
  the read had _delivered_, not which bound expired — on a silent on-air deck
  the dead-air limit is the shorter of the two and so always the one that fires,
  dead mount or not, and "the share is gone" and "the share is too slow to open
  a show with" are different problems. **The read is not cancelled** — it cannot be, and one
  that is merely slow is worth finishing. The event says the load was
  `abandoned`, which is what has the playlist return the track to the head of
  the queue rather than drop it; back in the window, its bytes are accepted
  when they land and it is instant to play at the next track change. What it
  costs is its turn, not the file — see [playlist.md](./playlist.md#outages).

A read that fails or times out emits `{role}:load-failed`, which the playlist
engine turns into skip-to-cached and a retry timer — see
[playlist.md](./playlist.md#outages).

What a deck reads on a miss is **offered to the cache** when it lands, so bytes
the share has already sent are not asked for a second time. The offer is refused
for a track outside the window — that is what keeps residency a playlist window
rather than an LRU — and nothing is lost by refusing, because the deck holds its
own copy for the life of the load.

### The prefetch cache

`audio/cache.rs` keeps whole track files resident in RAM, keyed by track id, so
a deck load finds its bytes already fetched. Residency is a **whole-playlist
window**, not an LRU: the playlist service pushes the upcoming ids (current
first, then playlist order), entries outside the latest window are evicted at
once, and the window is walked front-first until `MAX_CACHE_BYTES` is reached.
So the whole playlist is attempted and the nearest tracks win.

One background worker fetches missing entries sequentially, never in parallel —
hammering a network share with concurrent reads is how a share that was merely
slow becomes a share that is down.

**The rule belongs to the share, not to that worker.** The cache therefore holds
an **in-flight set**: every reader of a library file claims an id before it
reads, so one file never crosses the share twice at once. A deck that misses an
id someone else is already reading into the window waits for that read rather
than starting its own — never slower than a read that begins later, and bounded
by the deck's watchdog exactly as a read of its own would be. A claim therefore
carries the holder's **progress**: the waiter mirrors that byte count into its
own watchdog handle, so the wait ends when the holder stalls rather than when
the file turns out to be large. The analysis pass
keeps the same rule from the other side by reading on a single thread
([library.md](./library.md#the-analysis-pass)).

**Only for an id in the window.** Bytes for anything else are refused, so there
would be nothing to wait for: the waiter would read the file itself anyway, after
the other reader instead of alongside them. A cue audition of an unqueued track
is exactly that, and the editor reloads it on every edit — two reloads over a
slow share would otherwise take two read times end to end and trip the watchdog
on a share that is merely slow. Such a read takes no claim at all.

A claim is released when the read ends, including on a panic. A read _wedged_ on
a dead mount holds its claim until the OS finally errors it. The prefetch worker
waits on that entry rather than walking past it — nothing but a window push wakes
the worker, so an entry skipped once is not fetched at all if the reader holding
it then fails, and its bytes would go uncounted against the cap while in flight,
which would have the run read later entries that land only to be evicted. A new
window aborts the wait.

This is a per-file rule, not a global permit. The analysis reader, the prefetch
worker and a deck that missed can be pulling three _different_ files at once;
what cannot happen is the same file twice.

## Output devices

`audio/output.rs` owns one `OutputStream` per physical device. Every deck driven
by the same worker shares that one output, which is precisely what makes the
program bus a mixer instead of a set of independent streams.

The stream is opened **lazily and re-openably**: a failure at launch never
permanently disables playback (#259), and a device that comes back is picked up
by the next open. A fixed 4096-frame buffer is requested for predictable
latency, falling back to the device default when a host (CoreAudio, commonly)
rejects the request rather than killing the worker thread.

`audio/devices.rs` enumerates outputs through `cpal` and reports
`{ name, description, isDefault }`. The operator picks devices under _Settings →
Audio_; the choice is stored in `config.json` as a `DeviceRef { name,
description }` rather than an index or an opaque handle, so it survives the
device list changing shape. On boot the stored `name` is matched exactly, then
the `description` is matched as a fallback with a warning, and failing both the
main output falls back to the system default. The cue device is optional: with
none configured, the cue deck is simply not spawned.

### Exclusive output

Not implemented, and deliberately so. Exclusive mode means WASAPI's
`AUDCLNT_SHAREMODE_EXCLUSIVE` on Windows, `kAudioDevicePropertyHogMode` on
macOS, and opening `hw:N,M` directly on ALSA. cpal 0.16 — the version rodio
pulls — exposes none of the three: WASAPI is hardcoded to shared mode,
CoreAudio has no reference to hog mode in the crate, and ALSA opens whatever
name enumeration returned.

Shipping a toggle that does nothing would mislead an operator into believing
they have a device they do not, so there is no toggle. Three ways out exist if
it is ever prioritised — wait for upstream cpal (RustAudio/cpal#598, #743),
write a ~300 LOC per-platform adapter below cpal, or vendor a patched cpal —
and option one is the recommendation. A station needing bit-exact output today
can dedicate a USB interface to the app and accept shared-mode mixing.

Worth knowing regardless of the path: while exclusive mode is engaged, every
other application on that device goes silent (Slack calls, browser audio, system
alerts); exclusive mode requires the device's raw mix format, so a sample-rate
mismatch would need a resampling step the OS mixer currently provides for free;
and a hot-unplug that shared mode recovers from by rerouting would instead
require an explicit teardown and reopen.

## The cue deck

`audio/cue.rs`. One off-air deck, on its own output device, with its own output
stream, its own worker thread and its own `cue:*` topics. It is **not** on the
program bus — it is monitoring on a different physical device, not program
audio, and mixing it into the bus would put headphone content on air. It reuses
the deck worker for everything that is the same everywhere: whole-file reads,
the watchdog, the self-healing open.

It is the audition surface for the cue editor, in two modes that never mix:

- **Absolute** — plays the whole file, so an in-point can be scrubbed for.
- **Preview** — reloads with the draft markers applied and crops the waveform to
  the aired region.

Switching modes reloads the deck, because markers are applied at load time.
`cue_load` takes an optional `cuePoints` (so an unsaved draft can be auditioned)
and an `autoplay` flag; the deck parks its sink when the background read lands,
so a `Play` sent alongside a `Load` would be undone by it. Cueing and mode
switching stay parked, and the editor's _Play_, _Audition_ and pre-roll are the
only explicit asks. See [cue-points.md](./cue-points.md#authoring).

The cue deck writes no history and increments no play count. Only what reaches
the `main` role is an airing.

## ReplayGain

Every track is levelled to one reference so a sparse 1970s master and a modern
loudness-war master do not step on each other on air.

`rg_gain`, `rg_peak` and `rg_measured_at` live on `tracks`, **measured by the
analysis pass** (`audio_measure/waveform.rs::analyze`, one decode that also
yields the RMS curve, the automatic-cue windows and the tempo) rather than read
from
`replaygain_track_gain` tags. Two reasons:

- Most station libraries are largely untagged. Normalising only the tagged half
  would pull those tracks toward reference while the rest stayed at full scale —
  a level split where there was none.
- Tags are not comparable with each other anyway. ReplayGain 1.0 (89 dB
  reference) and 2.0 (-18 LUFS, EBU R128) write the same tag name from different
  targets.

`rg_measured_at` is what "measured" means, because a silent or very short file
legitimately has no gain: a `NULL` gain with a timestamp is a measurement, a
`NULL` gain without one is work not done yet.

`audio_measure/loudness.rs` owns the arithmetic — `TARGET_LUFS = -18.0`, the gain that
brings a measurement to it, and the linear factor that gain earns once clamped
so the loudest sample lands no higher than full scale. A track already peaking
near 1.0 cannot be turned up; that is ordinary ReplayGain behaviour.

Two invariants:

- **`Cmd::Load` carries an already-resolved linear factor**, exactly as it
  carries concrete `CuePoints`. The deck worker never consults the library.
- **The factor is applied at the source**, in `append_span` — not through
  `sink.set_volume()`, for the same reason the fade envelope is not (below) —
  and therefore before the bus mixer sums the decks, so a handover between
  tracks mastered at different levels crossfades correctly.

The same function serves the cue deck, whose headphone output never passes the
station's processing chain.

The setting is `player.replayGain` (`off` / `track`), and
`audio/levelling.rs::factor` is the one place it meets a measurement — the split
that keeps `audio_measure/` free of settings. There is deliberately no
album mode: a radio playlist is a sequence of singles, and album gain would
reintroduce exactly the between-track level differences track gain exists to
remove. See #80.

## The fade envelope

A track's stored fade-in and fade-out are a **source-level** envelope,
`Enveloped<I>` in `audio/envelope.rs`, not a `sink.set_volume()` ramp.

Stored fades are "at position X through position Y" operations, keyed to
absolute file position. The live fade buttons are "from now, over N ms"
operations on the deck. Routing both through one value makes them fight — a
live fade fired while a track is inside its own stored fade-out clobbers it,
last writer per tick wins. As a source envelope multiplied by a sink gain they
compose at different stages, with no priority rule to get wrong. The envelope is
also sample-accurate rather than stepped at the worker's 50 ms tick.

`gain_at(pos, cue)` has five branches: before the in-point, the ramp up, the
body, the ramp down, past the out-point. A ramp whose two positions coincide has
zero width and contributes nothing, and a track with no ramps at all is handed
to the sink unwrapped.

(rodio 0.21 cannot express an outro ramp on its own: `fade_out` and
`linear_gain_ramp` both ramp from the source's start, and
`TakeDuration::set_filter_fadeout` fades across the entire take.)

## Durations mean air time

Every duration that crosses the Tauri boundary is **air time** — `cueOut −
cueIn`, via `airDuration()` — not file length. A trimmed track is simply a
shorter track to the renderer.

Library rows, playlist rows, the history list, both decks and the now-playing
webhook all report air time. So do the Upcoming tab total and the main deck's
remaining-on-air countdown, both of which stop at the first stop marker. The
toolbar's library _Playtime_ figure alone stays file time, because it answers
"how much audio do I have", not "how long will this play".

`TrackTooltip` carries the file length too when the two differ. The renderer
never optimistically shows file time while a load is in flight, because the deck
reports air time and the two would visibly disagree mid-load.

Position works the same way: `0` on the air timeline is the first audible
sample. Only the player worker knows source-absolute positions.

## Seek

`audio/player.rs` reloads the source on seek — there is no seek on a live
`Sink`.

`append_span` then seeks in two stages:

1. `try_seek` — the container-level binary search — to roughly 200 ms short of
   the target.
2. `skip_duration` — sample iteration — for the remainder.

The second stage is what makes a stored marker land sample-exactly even in
formats where symphonia estimates the seek position from the bitrate. A
`try_seek` that fails outright falls back to `skip_duration` from zero.

`seek_offset + sink.get_pos()`, less `cue_in`, is what keeps `{role}:time`
accurate afterwards.

## Events

Deck events are role-mapped: whichever deck holds `main` emits `main-deck:*`,
the armed one `arm-deck:*`, an outgoing one `tail-deck:*`. The cue deck emits
`cue:*`. Per deck:

| topic                       | meaning                                         |
| --------------------------- | ----------------------------------------------- |
| `{role}:time`               | air-timeline position, 10 Hz                    |
| `{role}:duration`           | air time of the loaded track                    |
| `{role}:pause-state`        | paused / playing                                |
| `{role}:ended`              | the track reached its cue out or the file's end |
| `{role}:loaded`             | bytes are in the sink                           |
| `{role}:load-failed`        | the read failed, or was `abandoned` to a bound  |
| `{role}:buffering`          | waiting on a read                               |
| `{role}:error`              | anything else the worker could not recover from |
| `{role}:output-unavailable` | the output device could not be opened           |

`program:roles`, `program:handover` and `program:faded-out` belong to the bus —
see [program-bus.md](./program-bus.md#events-and-commands).

## Code map

Playback. What a decode _measures_ — the waveform curve, the loudness numbers,
the level envelope, the tempo and the extension table — is `audio_measure/`; see
[audio-measure.md](./audio-measure.md).

| file                  | holds                                                       |
| --------------------- | ----------------------------------------------------------- |
| `audio/player.rs`     | `Cmd`, `Topics`, whole-file read + retry + watchdog, decode |
| `audio/cache.rs`      | the prefetch window, its fetch worker, the in-flight claims |
| `audio/output.rs`     | one `OutputStream` per device, self-healing open            |
| `audio/devices.rs`    | cpal enumeration, `DeviceRef` resolution                    |
| `audio/deck.rs`       | one `Sink` per deck plus the worker loop over a deck set    |
| `audio/bus.rs`        | the program bus                                             |
| `audio/cue.rs`        | the cue deck                                                |
| `audio/cue_points.rs` | the five markers, resolved against a decoded duration       |
| `audio/envelope.rs`   | `Enveloped<I>`, `gain_at`                                   |
| `audio/levelling.rs`  | `factor` — the ReplayGain setting over a measurement        |
