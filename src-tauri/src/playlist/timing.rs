//! Pure, occurrence-based projection of the playlist's start times.

use serde::{Deserialize, Serialize};

/// One host-clock and monotonic-clock sample captured with transport state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClockSample {
    /// Elapsed seconds from the process's monotonic clock.
    pub monotonic_seconds: f64,
    /// The host clock in milliseconds since the Unix epoch.
    pub unix_ms: f64,
}

/// An upcoming occurrence and its resolved timing inputs.
#[derive(Clone, Debug, PartialEq)]
pub struct TimingItem {
    /// Queue occurrence identity, independent of the library track identity.
    pub occurrence_id: String,
    /// The audio span or Stop plan used for this occurrence.
    pub kind: TimingKind,
}

/// Timing facts resolved by the playlist's existing cue rules.
#[derive(Clone, Debug, PartialEq)]
pub enum TimingKind {
    /// A track's positions are measured from its resolved Cue In.
    Track {
        /// Resolved Cue Out minus Cue In, when known.
        air_duration: Option<f64>,
        /// Resolved Next Start minus Cue In; absent means Cue Out.
        next_start: Option<f64>,
        /// Playback already knows it will skip this occurrence.
        unavailable: bool,
    },
    /// A presenter-controlled pause; no plan is an unknown timing barrier.
    Stop {
        /// The optional pause plan, in seconds.
        planned_duration_seconds: Option<f64>,
    },
}

/// Whether transitions use Next Start or wait until Cue Out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimingMode {
    /// Track-to-track transitions may overlap.
    Auto,
    /// Every next item is estimated to start at Cue Out.
    Manual,
}

/// One coherent observation of the current audio instance.
#[derive(Clone, Debug, PartialEq)]
pub struct CurrentTiming {
    /// The queue occurrence that became this airing.
    pub occurrence_id: String,
    /// Current position in air seconds, captured with the clock sample.
    pub position: f64,
    /// Resolved air duration, when known.
    pub air_duration: Option<f64>,
    /// Resolved Next Start in air seconds; absent means Cue Out.
    pub next_start: Option<f64>,
    /// Time until the other audible deck ends, or zero without a tail.
    pub tail_remaining: f64,
}

/// The transport facts that determine the next occurrence's start.
#[derive(Clone, Debug, PartialEq)]
pub enum TimingTransport {
    /// Audio has actually started; loading's optimistic playing flag is excluded.
    Playing(CurrentTiming),
    /// Paused, stopped or interrupted; the whole schedule slips while waiting.
    Waiting {
        /// Remaining lead before Upcoming; zero for an empty deck.
        remaining: Option<f64>,
        /// Remaining audible tail, if it constrains a resumed overlap.
        tail_remaining: f64,
    },
    /// A consumed Stop, whose plan never resumes playback by itself.
    Holding {
        /// Seconds until the planned deadline, or zero for an unplanned Stop.
        remaining_plan: f64,
    },
}

/// A stable schedule plus one shared, locally extrapolated delay.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistTiming {
    /// Upcoming occurrences in queue order, including unknown arrivals.
    pub entries: Vec<TimingEntry>,
    /// Frozen accumulated delay, preserving fractional seconds across waits.
    pub delay_seconds: f64,
    /// Wall-clock start of the running delay; a planned deadline may be future.
    pub delay_started_at_unix_ms: Option<f64>,
}

/// The fixed part of one occurrence's displayed ETA.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TimingEntry {
    /// Queue occurrence identity.
    pub occurrence_id: String,
    /// Absolute Unix second, truncated only after summing fractional spans.
    pub base_second: Option<i64>,
}

#[derive(Clone, Debug)]
struct Projected {
    item: TimingItem,
    start: Option<f64>,
    tail_end: Option<f64>,
}

#[derive(Clone, Debug)]
struct CurrentAnchor {
    occurrence_id: String,
    start: f64,
    tail_end: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WaitingKind {
    Playing,
    Waiting,
    Holding,
}

/// Retains fractional origins and delay across coherent transport observations.
#[derive(Clone, Debug, Default)]
pub struct Schedule {
    rows: Vec<Projected>,
    current: Option<CurrentAnchor>,
    mode: Option<TimingMode>,
    waiting_kind: Option<WaitingKind>,
    delay_seconds: f64,
    waiting_since: Option<ClockSample>,
    current_requirement: Option<(String, Option<f64>, Option<f64>, bool)>,
    waiting_requirement: Option<(Option<f64>, f64)>,
}

impl Schedule {
    /// Applies one deliberate host-clock correction without advancing delay.
    pub fn shift_wall(&mut self, delta_ms: f64) {
        if !delta_ms.is_finite() {
            return;
        }
        let seconds = delta_ms / 1000.0;
        for row in &mut self.rows {
            row.start = row.start.map(|start| start + seconds);
            row.tail_end = row.tail_end.map(|end| end + seconds);
        }
        if let Some(current) = &mut self.current {
            current.start += seconds;
            current.tail_end += seconds;
        }
        if let Some(since) = &mut self.waiting_since {
            since.unix_ms += delta_ms;
        }
    }

    /// Revises the projection without using I/O or a clock of its own.
    ///
    /// Set `force_reanchor` for seeks, early skips, changed current cues, actual
    /// output interruptions and deliberate clock corrections. Routine progress,
    /// successful automatic handovers and queue suffix edits leave it false.
    /// Consumers display `baseSecond + floor(delaySeconds + elapsedDelay)`;
    /// `elapsedDelay` is nonnegative even before a planned Stop's deadline.
    pub fn update(
        &mut self,
        items: &[TimingItem],
        mode: TimingMode,
        transport: &TimingTransport,
        clock: ClockSample,
        force_reanchor: bool,
    ) -> PlaylistTiming {
        let previous_waiting_kind = self.waiting_kind;
        let reanchor = force_reanchor || self.mode.is_some_and(|previous| previous != mode);
        if reanchor {
            *self = Self::default();
        }
        let kind = match transport {
            TimingTransport::Playing(_) => WaitingKind::Playing,
            TimingTransport::Waiting { .. } => WaitingKind::Waiting,
            TimingTransport::Holding { .. } => WaitingKind::Holding,
        };
        if self.waiting_kind != Some(kind) {
            if let Some(since) = self.waiting_since.take() {
                self.delay_seconds += positive(clock.monotonic_seconds - since.monotonic_seconds);
            }
            self.waiting_since = match transport {
                TimingTransport::Playing(_) => None,
                TimingTransport::Waiting { .. } => Some(clock),
                TimingTransport::Holding { remaining_plan } => {
                    let remaining = positive(*remaining_plan);
                    Some(ClockSample {
                        monotonic_seconds: clock.monotonic_seconds + remaining,
                        unix_ms: clock.unix_ms + remaining * 1000.0,
                    })
                }
            };
        }
        self.waiting_kind = Some(kind);
        self.mode = Some(mode);

        let delay = self.delay_seconds
            + self.waiting_since.map_or(0.0, |since| {
                positive(clock.monotonic_seconds - since.monotonic_seconds)
            });
        let now = clock.unix_ms / 1000.0 - delay;
        let old_current_id = self
            .current
            .as_ref()
            .map(|current| current.occurrence_id.clone());
        let mut consumed = false;
        let mut lead_changed = false;
        let lead = match transport {
            TimingTransport::Playing(current) => {
                let requirement = (
                    current.occurrence_id.clone(),
                    current.air_duration,
                    current.next_start,
                    next_is_track(items, 0),
                );
                self.waiting_requirement = None;
                let previous = self.rows.iter().find(|row| {
                    row.item.occurrence_id == current.occurrence_id && row.start.is_some()
                });
                if let Some(previous) = previous {
                    if let TimingKind::Track {
                        air_duration,
                        next_start,
                        ..
                    } = previous.item.kind
                    {
                        lead_changed = air_duration != current.air_duration
                            || next_start != current.next_start;
                    }
                    self.current = Some(CurrentAnchor {
                        occurrence_id: current.occurrence_id.clone(),
                        start: previous.start.unwrap_or(now),
                        tail_end: previous.tail_end.unwrap_or(now),
                    });
                    consumed = true;
                } else if self
                    .current
                    .as_ref()
                    .is_none_or(|anchor| anchor.occurrence_id != current.occurrence_id)
                {
                    lead_changed = true;
                    self.current = Some(CurrentAnchor {
                        occurrence_id: current.occurrence_id.clone(),
                        start: now - positive(current.position),
                        tail_end: now + positive(current.tail_remaining),
                    });
                }
                if old_current_id.as_deref() == Some(current.occurrence_id.as_str()) {
                    lead_changed |= self
                        .current_requirement
                        .as_ref()
                        .is_some_and(|previous| previous != &requirement);
                }
                self.current_requirement = Some(requirement);
                let anchor = self
                    .current
                    .as_ref()
                    .expect("playing establishes an anchor");
                let duration = valid(current.air_duration);
                let span =
                    transition_span(duration, current.next_start, mode, next_is_track(items, 0));
                (
                    span.zip(duration).map(|(span, duration)| {
                        (anchor.start + span)
                            .max(anchor.tail_end)
                            .min(anchor.start + duration)
                    }),
                    duration.map(|duration| anchor.start + duration),
                )
            }
            TimingTransport::Waiting {
                remaining,
                tail_remaining,
            } => {
                let requirement = (*remaining, *tail_remaining);
                lead_changed = previous_waiting_kind == Some(WaitingKind::Waiting)
                    && self
                        .waiting_requirement
                        .as_ref()
                        .is_some_and(|previous| previous != &requirement);
                self.waiting_requirement = Some(requirement);
                (
                    valid(*remaining)
                        .map(|remaining| now + remaining.max(positive(*tail_remaining))),
                    Some(now + positive(*tail_remaining)),
                )
            }
            TimingTransport::Holding { remaining_plan } => {
                consumed = previous_waiting_kind != Some(WaitingKind::Holding);
                self.current = None;
                self.current_requirement = None;
                self.waiting_requirement = None;
                (Some(now + positive(*remaining_plan)), Some(now))
            }
        };
        let same_head = items
            .first()
            .zip(self.rows.first())
            .is_some_and(|(item, previous)| item.occurrence_id == previous.item.occurrence_id);
        let current_changed = match transport {
            TimingTransport::Playing(current) => {
                old_current_id.as_deref() != Some(&current.occurrence_id)
            }
            _ => false,
        };
        let retained = if !lead_changed && (same_head || consumed || current_changed) {
            items.first().and_then(|item| {
                self.rows
                    .iter()
                    .find(|row| row.item.occurrence_id == item.occurrence_id && row.start.is_some())
            })
        } else {
            None
        };
        let (mut start, mut tail_end) = retained.map_or(lead, |row| (row.start, row.tail_end));
        let mut rows = Vec::with_capacity(items.len());
        for (index, item) in items.iter().enumerate() {
            if is_unavailable(item) {
                rows.push(Projected {
                    item: item.clone(),
                    start: None,
                    tail_end,
                });
                continue;
            }
            rows.push(Projected {
                item: item.clone(),
                start,
                tail_end,
            });
            match &item.kind {
                TimingKind::Track {
                    air_duration,
                    next_start,
                    ..
                } => {
                    let duration = valid(*air_duration);
                    let end = start
                        .zip(duration)
                        .map(|(start, duration)| start + duration);
                    let span = transition_span(
                        duration,
                        *next_start,
                        mode,
                        next_is_track(items, index + 1),
                    );
                    start = start.zip(span).map(|(start, span)| {
                        (start + span)
                            .max(tail_end.unwrap_or(start))
                            .min(end.unwrap_or(f64::INFINITY))
                    });
                    tail_end = end;
                }
                TimingKind::Stop {
                    planned_duration_seconds,
                } => {
                    start = start
                        .zip(valid(*planned_duration_seconds))
                        .map(|(start, duration)| start + duration);
                    tail_end = start;
                }
            }
        }
        self.rows = rows;
        PlaylistTiming {
            entries: self
                .rows
                .iter()
                .map(|row| TimingEntry {
                    occurrence_id: row.item.occurrence_id.clone(),
                    base_second: row.start.map(|start| start.floor() as i64),
                })
                .collect(),
            delay_seconds: self.delay_seconds,
            delay_started_at_unix_ms: self.waiting_since.map(|since| since.unix_ms),
        }
    }
}

fn valid(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite() && *value >= 0.0)
}

fn positive(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

fn is_unavailable(item: &TimingItem) -> bool {
    matches!(
        item.kind,
        TimingKind::Track {
            unavailable: true,
            ..
        }
    )
}

fn next_is_track(items: &[TimingItem], from: usize) -> bool {
    items
        .iter()
        .skip(from)
        .find(|item| !is_unavailable(item))
        .is_some_and(|item| matches!(item.kind, TimingKind::Track { .. }))
}

fn transition_span(
    duration: Option<f64>,
    next_start: Option<f64>,
    mode: TimingMode,
    next_track: bool,
) -> Option<f64> {
    duration.map(|duration| {
        if mode == TimingMode::Auto && next_track {
            valid(next_start).unwrap_or(duration).min(duration)
        } else {
            duration
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock(seconds: f64) -> ClockSample {
        ClockSample {
            monotonic_seconds: seconds,
            unix_ms: (1000.0 + seconds) * 1000.0,
        }
    }

    fn track(id: &str, duration: f64, next_start: f64) -> TimingItem {
        TimingItem {
            occurrence_id: id.into(),
            kind: TimingKind::Track {
                air_duration: Some(duration),
                next_start: Some(next_start),
                unavailable: false,
            },
        }
    }

    fn stop(id: &str, plan: Option<f64>) -> TimingItem {
        TimingItem {
            occurrence_id: id.into(),
            kind: TimingKind::Stop {
                planned_duration_seconds: plan,
            },
        }
    }

    fn playing(id: &str, position: f64, duration: f64, next_start: f64) -> TimingTransport {
        TimingTransport::Playing(CurrentTiming {
            occurrence_id: id.into(),
            position,
            air_duration: Some(duration),
            next_start: Some(next_start),
            tail_remaining: 0.0,
        })
    }

    fn waiting(remaining: f64) -> TimingTransport {
        TimingTransport::Waiting {
            remaining: Some(remaining),
            tail_remaining: 0.0,
        }
    }

    fn seconds(timing: &PlaylistTiming) -> Vec<Option<i64>> {
        timing
            .entries
            .iter()
            .map(|entry| entry.base_second)
            .collect()
    }

    fn displayed(timing: &PlaylistTiming, at: ClockSample) -> Vec<Option<i64>> {
        let elapsed = timing
            .delay_started_at_unix_ms
            .map_or(0.0, |since| positive((at.unix_ms - since) / 1000.0));
        let delay = (timing.delay_seconds + elapsed).floor() as i64;
        timing
            .entries
            .iter()
            .map(|entry| entry.base_second.map(|base| base + delay))
            .collect()
    }

    #[test]
    fn skipping_the_same_head_can_change_the_current_transition_to_cue_out() {
        let mut schedule = Schedule::default();
        let mut items = [track("b", 3.0, 2.0), stop("stop", None)];
        let initial = schedule.update(
            &items,
            TimingMode::Auto,
            &playing("a", 0.0, 10.0, 5.0),
            clock(0.0),
            false,
        );
        assert_eq!(seconds(&initial), vec![Some(1005), Some(1008)]);
        if let TimingKind::Track { unavailable, .. } = &mut items[0].kind {
            *unavailable = true;
        }
        let skipped = schedule.update(
            &items,
            TimingMode::Auto,
            &playing("a", 2.0, 10.0, 5.0),
            clock(2.0),
            false,
        );
        assert_eq!(seconds(&skipped), vec![None, Some(1010)]);
    }

    #[test]
    fn decoded_current_timing_revises_only_the_affected_future() {
        let mut schedule = Schedule::default();
        let items = [track("b", 3.0, 2.0), track("c", 3.0, 2.0)];
        let initial = schedule.update(
            &items,
            TimingMode::Auto,
            &playing("a", 0.0, 100.0, 100.0),
            clock(0.0),
            false,
        );
        assert_eq!(seconds(&initial)[0], Some(1100));
        let decoded = schedule.update(
            &items,
            TimingMode::Auto,
            &playing("a", 1.0, 80.0, 80.0),
            clock(1.0),
            false,
        );
        assert_eq!(seconds(&decoded)[0], Some(1080));
    }

    #[test]
    fn a_parked_restore_adopts_the_coherently_observed_remaining_span() {
        let mut schedule = Schedule::default();
        let items = [track("b", 3.0, 2.0)];
        schedule.update(
            &items,
            TimingMode::Auto,
            &TimingTransport::Waiting {
                remaining: Some(100.0),
                tail_remaining: 0.0,
            },
            clock(0.0),
            false,
        );
        let restored = schedule.update(
            &items,
            TimingMode::Auto,
            &TimingTransport::Waiting {
                remaining: Some(50.0),
                tail_remaining: 0.0,
            },
            clock(0.05),
            false,
        );
        assert_eq!(seconds(&restored)[0], Some(1050));
    }

    #[test]
    fn fractions_accumulate_before_absolute_seconds_are_truncated() {
        let mut schedule = Schedule::default();
        let items = [
            track("a", 0.6, 0.6),
            track("b", 0.6, 0.6),
            track("c", 0.6, 0.6),
        ];
        let timing = schedule.update(&items, TimingMode::Auto, &waiting(0.0), clock(0.4), false);
        assert_eq!(seconds(&timing), vec![Some(1000), Some(1001), Some(1001)]);
    }

    #[test]
    fn delayed_and_reordered_progress_never_moves_a_playing_origin() {
        let mut schedule = Schedule::default();
        let items = [track("b", 2.6, 2.6), track("c", 2.6, 2.6)];
        let initial = schedule.update(
            &items,
            TimingMode::Auto,
            &playing("a", 0.0, 5.6, 5.6),
            clock(0.3),
            false,
        );
        for (time, position) in [(1.0, 0.7), (2.5, 1.2), (3.0, 0.2), (4.0, 3.7)] {
            let update = schedule.update(
                &items,
                TimingMode::Auto,
                &playing("a", position, 5.6, 5.6),
                clock(time),
                false,
            );
            assert_eq!(seconds(&update), seconds(&initial));
        }
    }

    #[test]
    fn successful_handovers_keep_fractional_occurrence_origins() {
        let mut schedule = Schedule::default();
        let items = [
            track("b", 2.6, 2.6),
            track("c", 2.6, 2.6),
            track("d", 2.6, 2.6),
        ];
        let initial = schedule.update(
            &items,
            TimingMode::Auto,
            &playing("a", 0.0, 2.6, 2.6),
            clock(0.3),
            false,
        );
        let handover = schedule.update(
            &items[1..],
            TimingMode::Auto,
            &playing("b", 0.0, 2.6, 2.6),
            clock(3.1),
            false,
        );
        assert_eq!(seconds(&handover), seconds(&initial)[1..]);
        let second = schedule.update(
            &items[2..],
            TimingMode::Auto,
            &playing("c", 0.0, 2.6, 2.6),
            clock(5.9),
            false,
        );
        assert_eq!(seconds(&second), seconds(&initial)[2..]);
    }

    #[test]
    fn repeated_library_tracks_have_independent_occurrences() {
        let mut schedule = Schedule::default();
        let items = [
            track("track-7-first", 3.7, 3.7),
            track("track-7-second", 3.7, 3.7),
        ];
        let initial = schedule.update(
            &items,
            TimingMode::Auto,
            &playing("other", 0.0, 2.7, 2.7),
            clock(0.0),
            false,
        );
        let next = schedule.update(
            &items[1..],
            TimingMode::Auto,
            &playing("track-7-first", 0.0, 3.7, 3.7),
            clock(3.0),
            false,
        );
        assert_eq!(next.entries[0].occurrence_id, "track-7-second");
        assert_eq!(seconds(&next), seconds(&initial)[1..]);
    }

    #[test]
    fn a_short_overlap_waits_for_the_previous_tail() {
        let mut schedule = Schedule::default();
        let items = [
            track("b", 1.0, 0.5),
            track("c", 3.0, 1.0),
            track("d", 3.0, 1.0),
        ];
        let timing = schedule.update(
            &items,
            TimingMode::Auto,
            &playing("a", 0.0, 10.0, 2.0),
            clock(0.0),
            false,
        );
        assert_eq!(seconds(&timing), vec![Some(1002), Some(1003), Some(1004)]);
        let next = schedule.update(
            &items[1..],
            TimingMode::Auto,
            &playing("b", 0.0, 1.0, 0.5),
            clock(2.1),
            false,
        );
        assert_eq!(seconds(&next), seconds(&timing)[1..]);
    }

    #[test]
    fn a_tail_already_audible_constrains_the_first_handover() {
        let mut schedule = Schedule::default();
        let transport = TimingTransport::Playing(CurrentTiming {
            occurrence_id: "a".into(),
            position: 0.0,
            air_duration: Some(2.0),
            next_start: Some(0.5),
            tail_remaining: 4.0,
        });
        let timing = schedule.update(
            &[track("b", 2.0, 1.0)],
            TimingMode::Auto,
            &transport,
            clock(0.0),
            false,
        );
        assert_eq!(seconds(&timing), vec![Some(1002)]);
    }

    #[test]
    fn a_stop_arrives_at_cue_out_even_when_the_tail_would_run_longer() {
        let mut schedule = Schedule::default();
        let transport = TimingTransport::Playing(CurrentTiming {
            occurrence_id: "short-main".into(),
            position: 0.0,
            air_duration: Some(2.0),
            next_start: Some(0.5),
            tail_remaining: 4.0,
        });
        let timing = schedule.update(
            &[stop("stop", Some(5.0)), track("after", 3.0, 1.0)],
            TimingMode::Auto,
            &transport,
            clock(0.0),
            false,
        );
        assert_eq!(seconds(&timing), vec![Some(1002), Some(1007)]);
    }

    #[test]
    fn auto_uses_cue_out_before_a_stop_and_manual_always_does() {
        let items = [
            track("b", 4.0, 1.0),
            stop("stop", Some(2.0)),
            track("c", 4.0, 1.0),
        ];
        let mut auto = Schedule::default();
        let result = auto.update(
            &items,
            TimingMode::Auto,
            &playing("a", 0.0, 5.0, 2.0),
            clock(0.0),
            false,
        );
        assert_eq!(seconds(&result), vec![Some(1002), Some(1006), Some(1008)]);
        let mut manual = Schedule::default();
        let result = manual.update(
            &items,
            TimingMode::Manual,
            &playing("a", 0.0, 5.0, 2.0),
            clock(0.0),
            false,
        );
        assert_eq!(seconds(&result), vec![Some(1005), Some(1009), Some(1011)]);
        let first_stop = auto.update(
            &items[1..],
            TimingMode::Auto,
            &playing("a", 0.0, 5.0, 2.0),
            clock(0.0),
            true,
        );
        assert_eq!(seconds(&first_stop), vec![Some(1005), Some(1007)]);
    }

    #[test]
    fn unavailable_tracks_do_not_hide_or_bypass_stops() {
        let mut unavailable = track("gone", 100.0, 90.0);
        if let TimingKind::Track { unavailable, .. } = &mut unavailable.kind {
            *unavailable = true;
        }
        let items = [unavailable, stop("stop", None), track("later", 2.0, 1.0)];
        let mut schedule = Schedule::default();
        let result = schedule.update(
            &items,
            TimingMode::Auto,
            &playing("a", 0.0, 5.0, 2.0),
            clock(0.0),
            false,
        );
        assert_eq!(seconds(&result), vec![None, Some(1005), None]);
    }

    #[test]
    fn unknown_track_timing_hides_only_subsequent_arrivals() {
        let items = [
            TimingItem {
                occurrence_id: "unknown".into(),
                kind: TimingKind::Track {
                    air_duration: None,
                    next_start: Some(1.0),
                    unavailable: false,
                },
            },
            track("later", 2.0, 1.0),
        ];
        let mut schedule = Schedule::default();
        let result = schedule.update(&items, TimingMode::Auto, &waiting(0.0), clock(0.0), false);
        assert_eq!(seconds(&result), vec![Some(1000), None]);
    }

    #[test]
    fn reaching_an_unplanned_barrier_reveals_the_next_stretch() {
        let items = [
            stop("stop", None),
            track("b", 2.0, 2.0),
            track("c", 2.0, 2.0),
        ];
        let mut schedule = Schedule::default();
        let initial = schedule.update(
            &items,
            TimingMode::Auto,
            &playing("a", 0.0, 5.0, 2.0),
            clock(0.0),
            false,
        );
        assert_eq!(seconds(&initial), vec![Some(1005), None, None]);
        let holding = TimingTransport::Holding {
            remaining_plan: 0.0,
        };
        let reached = schedule.update(&items[1..], TimingMode::Auto, &holding, clock(5.0), false);
        assert_eq!(seconds(&reached), vec![Some(1005), Some(1007)]);
        let later = schedule.update(&items[1..], TimingMode::Auto, &holding, clock(8.0), false);
        assert_eq!(displayed(&later, clock(8.0)), vec![Some(1008), Some(1010)]);
    }

    #[test]
    fn planned_hold_delays_only_after_its_deadline() {
        let items = [
            stop("stop", Some(5.0)),
            track("b", 2.0, 2.0),
            track("c", 2.0, 2.0),
        ];
        let mut schedule = Schedule::default();
        let initial = schedule.update(
            &items,
            TimingMode::Auto,
            &playing("a", 0.0, 2.0, 2.0),
            clock(0.0),
            false,
        );
        let reached = schedule.update(
            &items[1..],
            TimingMode::Auto,
            &TimingTransport::Holding {
                remaining_plan: 5.0,
            },
            clock(2.0),
            false,
        );
        assert_eq!(seconds(&reached), seconds(&initial)[1..]);
        assert_eq!(
            displayed(&reached, clock(6.9)),
            vec![Some(1007), Some(1009)]
        );
        assert_eq!(
            displayed(&reached, clock(10.1)),
            vec![Some(1010), Some(1012)]
        );
        let resumed = schedule.update(
            &items[2..],
            TimingMode::Auto,
            &playing("b", 0.0, 2.0, 2.0),
            clock(10.1),
            false,
        );
        assert_eq!(displayed(&resumed, clock(10.1)), vec![Some(1012)]);
        assert!(resumed.delay_started_at_unix_ms.is_none());
    }

    #[test]
    fn repeated_fractional_pauses_use_one_shared_delay_and_freeze_without_jump() {
        let items = [track("b", 2.7, 2.7), track("c", 2.7, 2.7)];
        let mut schedule = Schedule::default();
        let original = schedule.update(
            &items,
            TimingMode::Auto,
            &playing("a", 0.0, 10.0, 10.0),
            clock(0.0),
            false,
        );
        for (start, end) in [(1.0, 1.45), (2.0, 2.45), (3.0, 3.45)] {
            let paused =
                schedule.update(&items, TimingMode::Auto, &waiting(9.0), clock(start), false);
            let before = displayed(&paused, clock(end));
            let resumed = schedule.update(
                &items,
                TimingMode::Auto,
                &playing("a", 1.0, 10.0, 10.0),
                clock(end),
                false,
            );
            assert_eq!(displayed(&resumed, clock(end)), before);
            assert_eq!(seconds(&resumed), seconds(&original));
        }
        let result = schedule.update(
            &items,
            TimingMode::Auto,
            &playing("a", 2.0, 10.0, 10.0),
            clock(4.0),
            false,
        );
        assert!((result.delay_seconds - 1.35).abs() < 0.000001);
        assert_eq!(displayed(&result, clock(4.0)), vec![Some(1011), Some(1013)]);
    }

    #[test]
    fn paused_current_includes_its_remaining_time_and_idle_starts_now() {
        let items = [track("b", 2.0, 2.0)];
        let mut paused = Schedule::default();
        let result = paused.update(&items, TimingMode::Auto, &waiting(4.0), clock(0.0), false);
        assert_eq!(displayed(&result, clock(3.0)), vec![Some(1007)]);
        let mut idle = Schedule::default();
        let result = idle.update(&items, TimingMode::Auto, &waiting(0.0), clock(0.0), false);
        assert_eq!(displayed(&result, clock(3.0)), vec![Some(1003)]);
    }

    #[test]
    fn changing_suffix_timing_does_not_move_its_prefix() {
        let mut items = vec![
            track("b", 2.7, 2.7),
            track("c", 2.7, 2.7),
            track("d", 2.7, 2.7),
        ];
        let mut schedule = Schedule::default();
        let initial = schedule.update(
            &items,
            TimingMode::Auto,
            &playing("a", 0.0, 5.7, 5.7),
            clock(0.2),
            false,
        );
        items[1] = track("c", 8.7, 8.7);
        let result = schedule.update(
            &items,
            TimingMode::Auto,
            &playing("a", 1.0, 5.7, 5.7),
            clock(2.0),
            false,
        );
        assert_eq!(seconds(&result)[..2], seconds(&initial)[..2]);
        assert_eq!(
            seconds(&result)[2],
            seconds(&initial)[2].map(|second| second + 6)
        );
    }

    #[test]
    fn removing_the_head_reprojects_from_the_current_anchor() {
        let items = [track("b", 2.0, 2.0), track("c", 2.0, 2.0)];
        let mut schedule = Schedule::default();
        schedule.update(
            &items,
            TimingMode::Auto,
            &playing("a", 0.0, 5.0, 5.0),
            clock(0.0),
            false,
        );
        let result = schedule.update(
            &items[1..],
            TimingMode::Auto,
            &playing("a", 1.0, 5.0, 5.0),
            clock(1.1),
            false,
        );
        assert_eq!(seconds(&result), vec![Some(1005)]);
    }

    #[test]
    fn seek_or_loading_interruption_explicitly_reanchors() {
        let items = [track("b", 2.0, 2.0), track("c", 2.0, 2.0)];
        let mut schedule = Schedule::default();
        schedule.update(
            &items,
            TimingMode::Auto,
            &playing("a", 0.0, 10.0, 10.0),
            clock(0.0),
            false,
        );
        let seek = schedule.update(
            &items,
            TimingMode::Auto,
            &playing("a", 8.0, 10.0, 10.0),
            clock(2.0),
            true,
        );
        assert_eq!(seconds(&seek), vec![Some(1004), Some(1006)]);
        let loading = schedule.update(&items, TimingMode::Auto, &waiting(0.0), clock(3.0), true);
        assert_eq!(
            displayed(&loading, clock(6.0)),
            vec![Some(1006), Some(1008)]
        );
        let resumed = schedule.update(
            &items[1..],
            TimingMode::Auto,
            &playing("b", 0.0, 2.0, 2.0),
            clock(6.0),
            false,
        );
        assert_eq!(displayed(&resumed, clock(6.0)), vec![Some(1008)]);
    }

    #[test]
    fn empty_queue_retains_the_current_origin_for_later_appends() {
        let mut schedule = Schedule::default();
        schedule.update(
            &[],
            TimingMode::Auto,
            &playing("a", 0.0, 10.0, 10.0),
            clock(0.0),
            false,
        );
        let result = schedule.update(
            &[track("b", 2.0, 2.0)],
            TimingMode::Auto,
            &playing("a", 1.0, 10.0, 10.0),
            clock(2.0),
            false,
        );
        assert_eq!(seconds(&result), vec![Some(1010)]);
    }

    #[test]
    fn midnight_is_an_absolute_second_and_wall_correction_shifts_once() {
        let mut schedule = Schedule::default();
        let items = [track("a", 0.6, 0.6), track("b", 0.6, 0.6)];
        let before_midnight = ClockSample {
            monotonic_seconds: 1.0,
            unix_ms: 86_399_750.0,
        };
        let initial = schedule.update(
            &items,
            TimingMode::Auto,
            &waiting(0.0),
            before_midnight,
            false,
        );
        assert_eq!(seconds(&initial), vec![Some(86399), Some(86400)]);
        schedule.shift_wall(-3_600_000.0);
        let corrected = ClockSample {
            monotonic_seconds: 2.0,
            unix_ms: 82_800_750.0,
        };
        let result = schedule.update(&items, TimingMode::Auto, &waiting(0.0), corrected, false);
        assert_eq!(seconds(&result), vec![Some(82799), Some(82800)]);
        assert_eq!(
            displayed(&result, corrected),
            vec![Some(82800), Some(82801)]
        );
    }
}
