//! The audio output a deck worker plays into.
//!
//! One `OutputStream` per physical device. Every deck driven by the same worker
//! shares this one output — that is what makes the program bus a mixer rather
//! than a set of independent streams — and therefore shares its self-healing
//! open logic: the stream is opened lazily, re-opened on demand, and a launch
//! failure never permanently disables playback (#259).

use anyhow::{Context, Result};
use rodio::mixer::Mixer;
use rodio::{OutputStream, OutputStreamBuilder};
use std::time::{Duration, Instant};

use super::devices::resolve_device;
use crate::persist::config::DeviceRef;

const AUDIO_BUFFER_FRAMES: u32 = 4096;

fn stream_builder(device: &Option<cpal::Device>) -> Result<OutputStreamBuilder> {
    match device {
        Some(d) => OutputStreamBuilder::from_device(d.clone()).context("from_device"),
        None => OutputStreamBuilder::from_default_device().context("from_default_device"),
    }
}

fn open_stream(device: Option<cpal::Device>) -> Result<OutputStream> {
    // Prefer a fixed buffer size for predictable latency, but not every device
    // (or host, e.g. CoreAudio) accepts `Fixed`. When it rejects the request the
    // whole player thread would otherwise die at startup, so fall back to the
    // device default buffer size instead of propagating the error.
    match stream_builder(&device)?
        .with_buffer_size(cpal::BufferSize::Fixed(AUDIO_BUFFER_FRAMES))
        .open_stream()
    {
        Ok(stream) => Ok(stream),
        Err(e) => {
            log::warn!(
                "open_stream: fixed buffer {} rejected ({e}); retrying with default buffer size",
                AUDIO_BUFFER_FRAMES
            );
            stream_builder(&device)?
                .open_stream()
                .context("open_stream")
        }
    }
}

/// Resolve the configured device (if any) and open an output stream on the audio
/// thread. A missing or unopenable configured device is **not fatal**: it falls
/// back to the system default, so a device that is renamed, unplugged, or
/// briefly held/absent at launch does not permanently disable playback (#259).
fn open_device(device: &Option<DeviceRef>) -> Result<OutputStream> {
    let resolved = match device {
        Some(r) => match resolve_device(r) {
            Some(d) => Some(d),
            None => {
                log::warn!(
                    "audio device '{}' not found among current outputs; using default",
                    r.description
                );
                None
            }
        },
        None => None,
    };

    let had_specific = resolved.is_some();
    match open_stream(resolved) {
        Ok(s) => Ok(s),
        // A configured device that resolves but will not open (e.g. held
        // exclusively by another app) falls back to the default device rather
        // than failing the whole command.
        Err(e) if had_specific => {
            log::warn!("configured audio device failed to open ({e}); falling back to default");
            open_stream(None).context("open default output")
        }
        Err(e) => Err(e).context("open default output"),
    }
}

pub(super) struct Output {
    device: Option<DeviceRef>,
    stream: Option<OutputStream>,
    /// Bumped on every successful open. A deck whose sink was built against an
    /// older generation is connected to a dropped mixer, so it rebuilds.
    generation: u64,
    /// Whether the output is currently believed openable. Tracked so the
    /// `output-unavailable` event fires only on transitions (no per-retry spam).
    /// Optimistic at start — nothing is reported until the first real failure.
    ok: bool,
    /// Last time an open was attempted while a load was waiting on it. Paces
    /// the idle retry to `retry_interval`.
    last_open_retry: Option<Instant>,
    retry_interval: Duration,
}

impl Output {
    pub(super) fn new(device: Option<DeviceRef>, retry_interval: Duration) -> Self {
        Self {
            device,
            stream: None,
            generation: 0,
            ok: true,
            last_open_retry: None,
            retry_interval,
        }
    }

    /// Open the stream if it is not already open. Cheap and infallible once open.
    pub(super) fn open(&mut self) -> Result<()> {
        if self.stream.is_some() {
            return Ok(());
        }
        let stream = open_device(&self.device)?;
        self.stream = Some(stream);
        self.generation = self.generation.wrapping_add(1);
        Ok(())
    }

    pub(super) fn is_open(&self) -> bool {
        self.stream.is_some()
    }

    /// The live mixer every deck's sink connects to. Cloned (an `Arc` bump)
    /// rather than borrowed so callers can keep using `&mut Output` alongside it.
    pub(super) fn mixer(&self) -> Option<Mixer> {
        self.stream.as_ref().map(|s| s.mixer().clone())
    }

    /// Mixer + generation without attempting an open — for commands that should
    /// act on an already-open output but never bring one up on their own.
    pub(super) fn current(&self) -> Option<(Mixer, u64)> {
        self.mixer().map(|m| (m, self.generation))
    }

    /// Record availability and report whether it changed, so the caller emits
    /// `output-unavailable` only on transitions.
    pub(super) fn set_ok(&mut self, ok: bool) -> bool {
        let changed = self.ok != ok;
        self.ok = ok;
        changed
    }

    pub(super) fn retry_due(&self, now: Instant) -> bool {
        open_retry_due(self.last_open_retry, now, self.retry_interval)
    }

    pub(super) fn mark_retry_now(&mut self) {
        self.last_open_retry = Some(Instant::now());
    }

    pub(super) fn clear_retry(&mut self) {
        self.last_open_retry = None;
    }
}

/// Decide whether the idle loop should attempt another output-open. Pure (clock
/// via `now`) so it is unit-testable. Fires immediately the first time
/// (`None`), then no more often than `OPEN_RETRY_INTERVAL`.
fn open_retry_due(last_open_retry: Option<Instant>, now: Instant, interval: Duration) -> bool {
    match last_open_retry {
        None => true,
        Some(last) => now.saturating_duration_since(last) >= interval,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::player::OPEN_RETRY_INTERVAL;

    #[test]
    fn open_retry_fires_first_time_then_paces() {
        let start = Instant::now();
        let before = start
            .checked_add(OPEN_RETRY_INTERVAL - Duration::from_millis(1))
            .unwrap();
        let after = start
            .checked_add(OPEN_RETRY_INTERVAL + Duration::from_millis(1))
            .unwrap();

        // Never attempted before: retry immediately.
        assert!(open_retry_due(None, start, OPEN_RETRY_INTERVAL));
        // Within the interval since the last attempt: hold off.
        assert!(!open_retry_due(Some(start), before, OPEN_RETRY_INTERVAL));
        // Interval elapsed: retry again.
        assert!(open_retry_due(Some(start), after, OPEN_RETRY_INTERVAL));
    }
}
