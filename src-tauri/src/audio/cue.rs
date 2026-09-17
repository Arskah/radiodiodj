//! The cue deck: one off-air deck on its own output device.
//!
//! Deliberately **not** on the program bus. It is monitoring on a different
//! physical device, not program audio, so it keeps its own output stream, its
//! own worker thread, and its own `cue:*` topics. It reuses the deck worker for
//! the parts that are the same everywhere — whole-file reads, the watchdog, the
//! self-healing open.

use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::thread;
use tauri::{AppHandle, Emitter};

use super::cache::Cache;
use super::deck::{run, Deck, DeckEvents, DeckRole, DeckSet, DeckSlot};
use super::output::Output;
use super::player::{Cmd, PlayerTuning, Topics};
use crate::persist::config::DeviceRef;

pub struct CueDeck {
    tx: Sender<(DeckRole, Cmd)>,
}

impl CueDeck {
    pub fn spawn(
        app: AppHandle,
        device: Option<DeviceRef>,
        cache: Arc<Cache>,
        tuning: PlayerTuning,
    ) -> Self {
        let (tx, rx) = channel();
        // The cue deck has no roles to move; it is addressed as the worker's
        // only deck, which occupies role index 0.
        let events = vec![DeckEvents::new("cue")];
        let decks = vec![Deck::new(DeckSlot::A, DeckRole::Main)];
        thread::spawn(move || {
            // The output stream is opened on the worker thread and never
            // leaves it: `cpal::Stream` is not `Send` on every host.
            let output = Output::new(device, tuning.open_retry_interval);
            let set = DeckSet {
                decks,
                events,
                roles_topic: None,
                handover_topic: None,
                faded_out_topic: None,
            };
            if let Err(e) = run(app.clone(), rx, output, set, cache, tuning) {
                log::error!("cue deck thread exited: {}", e);
                let _ = app.emit(&Topics::new("cue").error, e.to_string());
            }
        });
        Self { tx }
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send((DeckRole::Main, cmd));
    }
}
