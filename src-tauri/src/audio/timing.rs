//! Coherent transport observations captured by the program-bus worker.

use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Meaningful transport observations; routine playhead events remain separate.
pub const TRANSPORT_EVENT: &str = "program:transport-timing";

struct Clock {
    origin: Instant,
    wall_ms: f64,
}

fn host_wall_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        * 1000.0
}

fn clock() -> &'static Mutex<Clock> {
    static CLOCK: OnceLock<Mutex<Clock>> = OnceLock::new();
    CLOCK.get_or_init(|| {
        Mutex::new(Clock {
            origin: Instant::now(),
            wall_ms: host_wall_ms(),
        })
    })
}

/// A single capture on the application’s monotonic and wall-anchored clocks.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capture {
    /// Seconds since the application clock was established.
    pub monotonic_seconds: f64,
    /// Unix milliseconds, advanced by monotonic elapsed time.
    pub unix_ms: f64,
}

/// Sample both clocks together; receipt of an observation never restarts time.
pub fn capture() -> Capture {
    let clock = clock().lock().unwrap_or_else(|error| error.into_inner());
    let elapsed = clock.origin.elapsed().as_secs_f64();
    Capture {
        monotonic_seconds: elapsed,
        unix_ms: clock.wall_ms + elapsed * 1000.0,
    }
}

/// Adopt a deliberate host-clock correction once, returning its wall offset.
/// Monotonic elapsed time, including held Stops, is unchanged.
pub fn correct_host_clock() -> f64 {
    let mut clock = clock().lock().unwrap_or_else(|error| error.into_inner());
    let projected = clock.wall_ms + clock.origin.elapsed().as_secs_f64() * 1000.0;
    let difference = host_wall_ms() - projected;
    if difference.abs() < 1000.0 {
        return 0.0;
    }
    clock.wall_ms += difference;
    difference
}

/// One atomically observed main-deck transport state, including its tail.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Observation {
    /// Strict worker emission order, used to reject delayed observations.
    pub sequence: u64,
    /// Physical deck and load generation, distinct even for repeated tracks.
    pub instance_id: String,
    /// Queue occurrence supplied with the load; absent on the cue deck.
    pub occurrence_id: Option<String>,
    /// Library identity, for validating the current airing.
    pub track_id: Option<i64>,
    /// Time captured on the worker, rather than its delivery time.
    pub captured: Capture,
    /// Air seconds at capture.
    pub position_seconds: f64,
    /// Decoded and resolved air duration.
    pub duration_seconds: Option<f64>,
    /// Resolved next-start marker in air seconds.
    pub next_start_seconds: Option<f64>,
    /// Actual source playback, never an optimistic loading report.
    pub playing: bool,
    /// File/output loading or interruption.
    pub buffering: bool,
    /// Loading has outlasted two normal worker turns, or the output is absent.
    pub interrupted: bool,
    /// Remaining audible tail that occupies the second deck.
    pub tail_remaining_seconds: f64,
    /// A seek changes the schedule even when playback state stays the same.
    pub seek_revision: u64,
}
