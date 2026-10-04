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

/// Bind what can now be bound, then send the list. Run wherever the library
/// itself moved — a scan or the analysis pass changing state, a purge — which
/// is when a fingerprint may have been written or a missing count changed.
pub fn refresh(app: &AppHandle, db: &Db) {
    match db.bind_saved_entries() {
        Ok(0) => {}
        Ok(n) => log::info!("saved playlists: bound {n} entries to tracks"),
        Err(e) => log::error!("saved playlists: binding failed: {e:#}"),
    }
    emit(app, db);
}

/// Send the list as it stands. Called after every change to a saved playlist.
pub fn emit(app: &AppHandle, db: &Db) {
    let _order = EMIT.lock();
    match db.saved_playlists() {
        Ok(list) => {
            let _ = app.emit(SAVED_PLAYLISTS_EVENT, &list);
        }
        Err(e) => log::error!("saved playlists: {e:#}"),
    }
}
