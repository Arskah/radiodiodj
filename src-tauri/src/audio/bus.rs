//! The program bus: every on-air deck summed into the main output device.
//!
//! One `OutputStream`, one `Sink` per deck on its mixer, one worker thread
//! driving them all. That is what lets two decks be audible at once — the
//! precondition for handover — and it makes the deck count incidental: adding a
//! soundboard or a sweeper deck is another entry in the `Vec`, not another
//! rewrite.
//!
//! The `main` role moves between the decks at the outgoing track's `nextStart`
//! — see `docs/program-bus.md#handover`. The playlist engine authorises a
//! handover by arm-loading the next item; this worker only times it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::thread;
use tauri::{AppHandle, Emitter};

use super::cache::Cache;
use super::deck::{run, Deck, DeckEvents, DeckRole, DeckSet, DeckSlot};
use super::output::Output;
use super::player::{Cmd, PlayerTuning, Topics};
use crate::persist::config::DeviceRef;

/// Topic the slot→role snapshot is emitted on. Debugging and future UI; the
/// transport and Now playing speak role-mapped `main-deck:*` instead.
pub const ROLES_EVENT: &str = "program:roles";

/// Topic a move of the `main` role is announced on. The playlist engine
/// reconciles against it: the queued item is consumed, the airing counted, and
/// the outgoing track handed to history.
pub const HANDOVER_EVENT: &str = "program:handover";

pub struct ProgramBus {
    tx: Sender<(DeckRole, Cmd)>,
    /// Mirrors the last `pause-state` the `main` role emitted, whichever deck
    /// held it. A renderer that attaches after the event — a reload, or a
    /// session restored before the window existed — reads the truth instead of
    /// assuming.
    main_playing: Arc<AtomicBool>,
}

impl ProgramBus {
    pub fn spawn(
        app: AppHandle,
        device: Option<DeviceRef>,
        cache: Arc<Cache>,
        tuning: PlayerTuning,
    ) -> Self {
        let (tx, rx) = channel();
        // Indexed by `DeckRole as usize`.
        let events = vec![
            DeckEvents::new("main-deck"),
            DeckEvents::new("arm-deck"),
            DeckEvents::new("tail-deck"),
        ];
        let main_playing = Arc::clone(&events[DeckRole::Main as usize].playing);
        let decks = vec![
            Deck::new(DeckSlot::A, DeckRole::Main),
            Deck::new(DeckSlot::B, DeckRole::Arm),
        ];
        thread::spawn(move || {
            // The output stream is opened on the worker thread and never
            // leaves it: `cpal::Stream` is not `Send` on every host.
            let output = Output::new(device, tuning.open_retry_interval);
            let set = DeckSet {
                decks,
                events,
                roles_topic: Some(ROLES_EVENT),
                handover_topic: Some(HANDOVER_EVENT),
            };
            if let Err(e) = run(app.clone(), rx, output, set, cache, tuning) {
                log::error!("program bus thread exited: {}", e);
                let _ = app.emit(&Topics::new("main-deck").error, e.to_string());
            }
        });
        Self { tx, main_playing }
    }

    /// Send a transport command to whichever deck currently holds `main`.
    pub fn send_main(&self, cmd: Cmd) {
        let _ = self.tx.send((DeckRole::Main, cmd));
    }

    /// Send a command to whichever deck is armed. Used to park the next
    /// playlist item ahead of a handover; never for anything audible.
    pub fn send_arm(&self, cmd: Cmd) {
        let _ = self.tx.send((DeckRole::Arm, cmd));
    }

    pub fn main_is_playing(&self) -> bool {
        self.main_playing.load(Ordering::SeqCst)
    }
}
