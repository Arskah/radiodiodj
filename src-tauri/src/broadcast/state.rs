use chrono::{DateTime, Utc};

use super::payload::{BroadcastTrack, Payload};

/// Pure FSM tracking what the broadcast service has last announced.
///
/// Inputs come from `BroadcastService` after listening to Tauri events:
/// - `set_pending(track)` — the track about to play on the main deck.
/// - `on_air(track)` — a handover put a track on air with no load of its own.
/// - `on_pause_state(paused)` — main deck pause-state transitions.
/// - `on_ended()` — main deck track ended naturally.
/// - `on_shutdown()` — app quit; fire final stop if currently playing.
///
/// Output is `Effect`, telling the service what to broadcast.
#[derive(Default, Debug)]
pub struct State {
    pending: Option<BroadcastTrack>,
    announced_track_id: Option<i64>,
}

#[derive(Debug, PartialEq)]
pub enum Effect {
    None,
    Fire(Payload),
}

impl State {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_pending(&mut self, track: BroadcastTrack) {
        self.pending = Some(track);
    }

    /// A track went on air without a load having just happened: the `main` role
    /// moved to a deck that was already playing.
    ///
    /// The one input that says "this is on air" outright. Pause-state is an
    /// inference, and at a handover it is a race — the bus worker emits the
    /// swap's pause-state before the playlist engine has reconciled — so the
    /// handover says so itself instead.
    pub fn on_air(&mut self, track: BroadcastTrack, now: DateTime<Utc>) -> Effect {
        self.pending = Some(track.clone());
        if self.announced_track_id == Some(track.id) {
            return Effect::None;
        }
        self.announced_track_id = Some(track.id);
        Effect::Fire(Payload::now_playing(track, now))
    }

    pub fn on_pause_state(&mut self, paused: bool, now: DateTime<Utc>) -> Effect {
        if paused {
            self.fire_stop_if_announced(now)
        } else if let Some(track) = self.pending.clone() {
            if self.announced_track_id != Some(track.id) {
                self.announced_track_id = Some(track.id);
                Effect::Fire(Payload::now_playing(track, now))
            } else {
                Effect::None
            }
        } else {
            Effect::None
        }
    }

    pub fn on_ended(&mut self, now: DateTime<Utc>) -> Effect {
        self.fire_stop_if_announced(now)
    }

    pub fn on_shutdown(&mut self, now: DateTime<Utc>) -> Effect {
        self.fire_stop_if_announced(now)
    }

    fn fire_stop_if_announced(&mut self, now: DateTime<Utc>) -> Effect {
        if self.announced_track_id.take().is_some() {
            Effect::Fire(Payload::stopped(now))
        } else {
            Effect::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: i64) -> BroadcastTrack {
        BroadcastTrack {
            id,
            title: format!("title-{id}"),
            artist: "a".into(),
            album: "alb".into(),
            genre: None,
            duration_sec: 100.0,
            content_type: "music".into(),
            path: format!("/p/{id}.mp3"),
        }
    }

    fn now() -> DateTime<Utc> {
        "2026-05-19T00:00:00Z".parse().unwrap()
    }

    fn unwrap_now_playing(e: Effect) -> BroadcastTrack {
        match e {
            Effect::Fire(Payload::NowPlaying(p)) => p.track,
            other => panic!("expected NowPlaying, got {other:?}"),
        }
    }

    fn unwrap_stopped(e: Effect) {
        match e {
            Effect::Fire(Payload::Stopped(_)) => (),
            other => panic!("expected Stopped, got {other:?}"),
        }
    }

    #[test]
    fn pause_state_false_without_pending_does_nothing() {
        let mut s = State::new();
        assert_eq!(s.on_pause_state(false, now()), Effect::None);
    }

    #[test]
    fn pause_state_false_with_pending_fires_now_playing() {
        let mut s = State::new();
        s.set_pending(t(1));
        let track = unwrap_now_playing(s.on_pause_state(false, now()));
        assert_eq!(track.id, 1);
    }

    #[test]
    fn same_track_playing_twice_does_not_re_fire() {
        let mut s = State::new();
        s.set_pending(t(1));
        unwrap_now_playing(s.on_pause_state(false, now()));
        // Same pending track + another play (e.g., seek reload) — dedupe.
        let again = s.on_pause_state(false, now());
        assert_eq!(again, Effect::None);
    }

    #[test]
    fn pause_after_play_fires_stop_then_resume_re_fires_now_playing() {
        let mut s = State::new();
        s.set_pending(t(1));
        unwrap_now_playing(s.on_pause_state(false, now()));

        unwrap_stopped(s.on_pause_state(true, now()));

        // Resume same track — should re-fire (state cleared on stop).
        let track = unwrap_now_playing(s.on_pause_state(false, now()));
        assert_eq!(track.id, 1);
    }

    #[test]
    fn pause_when_nothing_announced_does_nothing() {
        let mut s = State::new();
        assert_eq!(s.on_pause_state(true, now()), Effect::None);
    }

    #[test]
    fn ended_fires_stop_once() {
        let mut s = State::new();
        s.set_pending(t(1));
        unwrap_now_playing(s.on_pause_state(false, now()));
        unwrap_stopped(s.on_ended(now()));
        // Second ended without intervening play — silent.
        assert_eq!(s.on_ended(now()), Effect::None);
    }

    #[test]
    fn pending_swap_before_play_fires_new_track() {
        let mut s = State::new();
        s.set_pending(t(1));
        s.set_pending(t(2));
        let track = unwrap_now_playing(s.on_pause_state(false, now()));
        assert_eq!(track.id, 2);
    }

    #[test]
    fn track_change_fires_stop_then_new_now_playing() {
        let mut s = State::new();
        s.set_pending(t(1));
        unwrap_now_playing(s.on_pause_state(false, now()));

        // Track ends, new track loads, plays.
        unwrap_stopped(s.on_ended(now()));
        s.set_pending(t(2));
        let track = unwrap_now_playing(s.on_pause_state(false, now()));
        assert_eq!(track.id, 2);
    }

    #[test]
    fn shutdown_fires_stop_when_announced() {
        let mut s = State::new();
        s.set_pending(t(1));
        unwrap_now_playing(s.on_pause_state(false, now()));
        unwrap_stopped(s.on_shutdown(now()));
        assert_eq!(s.on_shutdown(now()), Effect::None);
    }

    #[test]
    fn shutdown_without_announcement_is_silent() {
        let mut s = State::new();
        assert_eq!(s.on_shutdown(now()), Effect::None);
    }

    #[test]
    fn set_pending_without_play_does_not_fire() {
        let mut s = State::new();
        s.set_pending(t(1));
        // No pause-state event yet — broadcast silent.
        assert_eq!(s.on_ended(now()), Effect::None);
    }
}
