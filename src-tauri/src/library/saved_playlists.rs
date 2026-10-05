//! The saved playlists list as the renderer mirrors it. The tables and their
//! queries are `db/saved_playlists.rs`; see `docs/saved-playlists.md`.

use parking_lot::Mutex;
use tauri::{AppHandle, Emitter};

use super::db::Db;

/// Topic the whole list of saved playlists is emitted on.
pub const SAVED_PLAYLISTS_EVENT: &str = "saved-playlists";

/// Held across the read and the emit, so two changes cannot deliver their
/// lists in the wrong order.
static EMIT: Mutex<()> = Mutex::new(());

/// Send the list as it stands. Called after every change to a saved playlist,
/// and after anything that moves a missing count: a scan, a purge.
pub fn emit(app: &AppHandle, db: &Db) {
    let _order = EMIT.lock();
    match db.saved_playlists() {
        Ok(list) => {
            let _ = app.emit(SAVED_PLAYLISTS_EVENT, &list);
        }
        Err(e) => log::error!("saved playlists: {e:#}"),
    }
}
