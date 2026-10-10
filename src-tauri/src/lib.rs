//! The backend: the Tauri command surface and the state every command
//! reaches through.
//!
//! The domains behind it live in their own modules — `audio`, `library`,
//! `playlist`, `broadcast`, `appearance`, `persist` and `admin`. What is here
//! is the boundary: one `#[tauri::command]` per renderer call, [`AppState`]
//! holding the handles they share, and [`run`] wiring the two together. See
//! `docs/architecture.md`.

use parking_lot::Mutex;
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

mod admin;
mod appearance;
mod audio;
mod audio_measure;
mod broadcast;
mod hub;
mod library;
mod persist;
mod playlist;
mod update;

use audio::cache::Cache;
use audio::devices::{list_output_devices, DeviceInfo};

/// The product name, as it appears in window titles and dialogs.
pub const APP_NAME: &str = "RadiodioDJ";

use admin::{AdminLock, AdminStatus};
use appearance::{Appearance, ThemeListing};
use audio::bus::ProgramBus;
use audio::cue::CueDeck;
use audio::cue_points::CuePoints;
use audio::player::{Cmd, PlayerTuning, RampDone};
use broadcast::{service::default_now_playing_dir, BroadcastService};
use library::check::LibraryCheck;
use library::db::{
    Db, LibraryStats, OpenError, Recalculated, SavedPlaylist, SavedPlaylistFile,
    SavedPlaylistSummary, Track, TrackMetadataUpdate,
};
use library::health::{FindingKind, Health, HealthReport};
use library::roots::{self, LibraryPath};
use library::saved_playlists;
use library::scan_state::{ScanState, ScanStatus, StartResult};
use library::scanner::now_ms;
use library::tag_backfill::TagBackfillJob;
use library::tag_write::TagWriter;
use library::waveform_scan::{WaveformJob, WaveformStatus};
use persist::config::{
    AppearanceConfig, Config, DeviceRef, LibraryRole, NowPlayingConfig, SharedLibrarySettings,
    TuningConfig,
};
use persist::session::{Session, SessionPlaylistItem, SessionState};
use playlist::{PlaylistService, Snapshot};
use std::time::Duration;
use update::{UpdateState, Updater};

/// Build the audio-player network-resilience tuning from the stored tuning
/// section. Values are already clamped on write, so the lists are non-empty.
fn player_tuning_from(t: &TuningConfig) -> PlayerTuning {
    PlayerTuning {
        read_watchdog_timeout: Duration::from_millis(t.player.read_watchdog_timeout_ms),
        dead_air_limit: Duration::from_millis(t.player.dead_air_limit_ms),
        open_retry_interval: Duration::from_millis(t.player.open_retry_interval_ms),
        read_retry_backoffs: t
            .player
            .read_retry_backoffs_ms
            .iter()
            .map(|ms| Duration::from_millis(*ms))
            .collect(),
    }
}

/// Keep a database written by a newer build untouched: hide the window, say
/// why, and quit once the operator acknowledges.
fn refuse_to_start(app: &AppHandle, refusal: &OpenError) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
    log::error!("{refusal}");
    for window in app.webview_windows().values() {
        let _ = window.hide();
    }
    let handle = app.clone();
    app.dialog()
        .message(format!(
            "{refusal}.\n\nInstall the newer version to open this library. \
             It has not been modified."
        ))
        .title(APP_NAME)
        .kind(MessageDialogKind::Error)
        .show(move |_| handle.exit(1));
}

/// Everything a command handler can reach, managed by Tauri.
///
/// The argument is named `app` in handlers, not `state`: a command argument
/// called `state` collides with the `State<AppState>` injection.
pub struct AppState {
    db: Arc<Db>,
    config: Arc<Config>,
    /// Admin mode. The invoke handler refuses admin-only commands while locked.
    admin: Arc<AdminLock>,
    session: Arc<Session>,
    scan: Arc<ScanState>,
    /// Background job that fills track waveforms after a metadata scan.
    waveform: Arc<WaveformJob>,
    /// Background job that fills tag columns a row predates. Separate from
    /// `waveform` because a file whose audio will not decode still has tags.
    tag_backfill: Arc<TagBackfillJob>,
    /// Missing tracks and duplicates, kept current for the renderer.
    health: Arc<Health>,
    /// Compares the disk with the library between scans.
    check: Arc<LibraryCheck>,
    /// Writes metadata edits into the files' tags, when enabled.
    tag_writer: Arc<TagWriter>,
    /// The mixer every on-air deck sums into, and the worker driving them.
    bus: Arc<ProgramBus>,
    /// Owner of the playlist and of everything that advances it.
    playlist: Arc<PlaylistService>,
    cue: Arc<Mutex<Option<CueDeck>>>,
    /// Shared prefetch byte cache, resident in both deck players.
    cache: Arc<Cache>,
    broadcast: Arc<BroadcastService>,
    /// Checks for a newer release, and installs one when asked.
    updater: Arc<Updater>,
    /// Set when this launch replaced a database from an older epoch, so the
    /// renderer can say why the library is rescanning.
    library_reset: bool,
    /// Set when this launch replaced the library to join a shared one, so the
    /// renderer can say why it is empty.
    library_joined: bool,
    /// This machine's part in a shared library.
    hub: Arc<hub::Service>,
    /// The per-user data directory, root of `themes/` and `branding/`.
    data_dir: std::path::PathBuf,
    app_handle: AppHandle,
}

#[derive(Serialize)]
struct SessionLoadResult {
    state: SessionState,
    tracks: Vec<Track>,
    library_reset: bool,
    library_joined: bool,
}

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Run `f` somewhere other than the thread the window is drawn from.
///
/// A command handler declared without `async` is the blocking kind: the macro
/// runs it inline on the IPC thread, which is the main thread. So a command
/// that waits on the database, the filesystem or the network freezes the window
/// for as long as it waits — and the library is usually a network share, where
/// "as long as it waits" has no useful bound.
///
/// On `spawn_blocking` rather than in an `async` command's body: a blocking
/// body holds one of the async runtime's worker threads, of which there is one
/// per core, so a handful of slow commands would take every other async command
/// down with them. The blocking pool exists for exactly this.
///
/// Callers clone the `Arc`s they need out of `AppState` before awaiting rather
/// than holding `State<'_, AppState>` across the await.
async fn blocking<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f).await.map_err(err)?
}

#[tauri::command(rename_all = "camelCase")]
async fn search(
    state: State<'_, AppState>,
    query: String,
    content_type: Option<String>,
    sort_by: Option<String>,
    sort_dir: Option<String>,
) -> Result<Vec<Track>, String> {
    let db = Arc::clone(&state.db);
    blocking(move || {
        db.search(
            &query,
            content_type.as_deref(),
            sort_by.as_deref(),
            sort_dir.as_deref(),
        )
        .map_err(err)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn get_track(state: State<'_, AppState>, id: i64) -> Result<Option<Track>, String> {
    let db = Arc::clone(&state.db);
    blocking(move || db.get_track(id).map_err(err)).await
}

#[tauri::command(rename_all = "camelCase")]
async fn get_tracks_by_ids(
    state: State<'_, AppState>,
    ids: Vec<i64>,
) -> Result<Vec<Track>, String> {
    let db = Arc::clone(&state.db);
    blocking(move || db.get_tracks_by_ids(&ids).map_err(err)).await
}

/// Show a track's file in the platform file manager (Finder, Explorer, or
/// whatever answers `org.freedesktop.FileManager1`). Takes an id, not a path,
/// so the renderer can only reveal files the library already knows.
///
/// Off the main thread for the `exists` check as much as the lookup: that is a
/// `stat` of the share, which a dead mount answers at its own pace.
#[tauri::command(rename_all = "camelCase")]
async fn reveal_track(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    blocking(move || {
        let info = db
            .get_track_load_info(id)
            .map_err(err)?
            .ok_or_else(|| format!("track {id} not found"))?;
        let path = std::path::Path::new(&info.path);
        if !path.exists() {
            return Err(format!("file not found: {}", info.path));
        }
        tauri_plugin_opener::reveal_item_in_dir(path).map_err(err)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn get_stats(state: State<'_, AppState>) -> Result<LibraryStats, String> {
    let db = Arc::clone(&state.db);
    blocking(move || db.get_stats().map_err(err)).await
}

#[tauri::command(rename_all = "camelCase")]
async fn track_played(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    blocking(move || db.increment_play_count(id).map_err(err)).await
}

/// Reads the library's own copy of its paths, which is memory, not the
/// database: hence sync.
#[tauri::command(rename_all = "camelCase")]
fn get_all_paths(state: State<'_, AppState>) -> BTreeMap<&'static str, Vec<LibraryPath>> {
    roots::list(&state.db)
}

/// Adding a library path rebuilds the health report, which is a pass over the
/// whole library — hence off the main thread, like every other `Health::refresh`
/// caller.
#[tauri::command(rename_all = "camelCase")]
async fn add_path(
    state: State<'_, AppState>,
    r#type: String,
    dir_path: String,
) -> Result<bool, String> {
    let db = Arc::clone(&state.db);
    let config = Arc::clone(&state.config);
    let health = Arc::clone(&state.health);
    blocking(move || {
        let added = roots::add(&db, &config, &r#type, &dir_path).map_err(err)?;
        health.refresh();
        Ok(added)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn remove_path(state: State<'_, AppState>, id: i64) -> Result<bool, String> {
    let db = Arc::clone(&state.db);
    let config = Arc::clone(&state.config);
    let health = Arc::clone(&state.health);
    blocking(move || {
        let removed = roots::remove(&db, &config, id).map_err(err)?;
        health.refresh();
        Ok(removed)
    })
    .await
}

/// Point a library path at another folder on this machine.
#[tauri::command(rename_all = "camelCase")]
async fn locate_path(
    state: State<'_, AppState>,
    id: i64,
    dir_path: String,
) -> Result<bool, String> {
    let db = Arc::clone(&state.db);
    let config = Arc::clone(&state.config);
    let health = Arc::clone(&state.health);
    blocking(move || {
        let located = roots::locate(&db, &config, id, &dir_path).map_err(err)?;
        health.refresh();
        Ok(located)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn load_session(app: State<'_, AppState>) -> Result<SessionLoadResult, String> {
    let session = Arc::clone(&app.session);
    let db = Arc::clone(&app.db);
    let library_reset = app.library_reset;
    let library_joined = app.library_joined;
    blocking(move || {
        let s = session.load();
        let mut ids: Vec<i64> = Vec::new();
        let mut seen: HashSet<i64> = HashSet::new();
        let item_ids = s.playlist_items.iter().filter_map(|item| match item {
            SessionPlaylistItem::Track { id, .. } => Some(id),
            SessionPlaylistItem::Stop => None,
        });
        for id in s
            .playlist_ids
            .iter()
            .chain(item_ids)
            .chain(s.current_track_id.iter())
        {
            if seen.insert(*id) {
                ids.push(*id);
            }
        }
        let tracks = db.get_tracks_by_ids(&ids).map_err(err)?;
        Ok(SessionLoadResult {
            state: s,
            tracks,
            library_reset,
            library_joined,
        })
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn save_session(app: State<'_, AppState>, state: SessionState) -> Result<(), String> {
    let session = Arc::clone(&app.session);
    blocking(move || session.save(state).map_err(err)).await
}

/// The playlist commands below are the renderer's only way to change what is
/// queued or on air. Each one queues a transition in the backend and the
/// resulting `program:playlist-state` snapshot is what the renderer draws.
///
/// They return the moment the transition is queued, not when it has run: the
/// work happens on the playlist's own thread, so a refill's SQL never lands on
/// the thread the window is drawn from. Nothing is lost by that — the snapshot
/// was always what the renderer read, and a command that cannot be carried out
/// is logged in the backend.
#[tauri::command(rename_all = "camelCase")]
fn playlist_sync(app: State<'_, AppState>) -> Snapshot {
    app.playlist.snapshot()
}

#[tauri::command(rename_all = "camelCase")]
fn playlist_add(app: State<'_, AppState>, id: i64, cue_points: Option<CuePoints>) {
    app.playlist.add(id, cue_points);
}

/// Insert at the head as next-up — the cue editor's _Use once_.
///
/// `cue_points` is an override for this one airing, which _Use once_ sends
/// when the draft differs from the track's radio edit.
/// Absent or `null` leaves the item referencing the track.
#[tauri::command(rename_all = "camelCase")]
fn playlist_add_front(app: State<'_, AppState>, id: i64, cue_points: Option<CuePoints>) {
    app.playlist.add_front(id, cue_points);
}

/// Insert ahead of the item at `index` — a library row dropped on the playlist.
/// An index past the end appends.
#[tauri::command(rename_all = "camelCase")]
fn playlist_insert(app: State<'_, AppState>, id: i64, index: usize) {
    app.playlist.insert(id, index);
}

/// Insert several tracks as one run, in the order given, ahead of the item at
/// `index` — a library selection. Absent or `null` appends, as does an index
/// past the end.
#[tauri::command(rename_all = "camelCase")]
fn playlist_add_many(app: State<'_, AppState>, ids: Vec<i64>, index: Option<usize>) {
    app.playlist.add_many(ids, index);
}

/// What an append of a saved playlist did: how many tracks it queued, and how
/// many entries it had to leave out for want of a track.
#[derive(serde::Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct SavedAppend {
    added: usize,
    skipped: usize,
}

/// Append a saved playlist's tracks, in order; with `weave`, brought up to the
/// interleave cadence. Reads before it queues, which is why it alone among the
/// `playlist_*` commands has a result. See `docs/saved-playlists.md`.
#[tauri::command(rename_all = "camelCase")]
async fn playlist_add_saved(
    state: State<'_, AppState>,
    id: i64,
    weave: bool,
) -> Result<SavedAppend, String> {
    let db = Arc::clone(&state.db);
    let playlist = Arc::clone(&state.playlist);
    blocking(move || {
        let offered = db
            .saved_playlist_tracks(id)
            .map_err(err)?
            .ok_or_else(|| format!("no saved playlist {id}"))?;
        let added = offered.tracks.len();
        playlist.add_saved(offered.tracks, weave);
        Ok(SavedAppend {
            added,
            skipped: offered.unmatched,
        })
    })
    .await
}

/// Keep the upcoming tracks as a new saved playlist. Stop markers and item
/// overrides are not carried.
#[tauri::command(rename_all = "camelCase")]
async fn playlist_save_as(
    handle: AppHandle,
    state: State<'_, AppState>,
    name: String,
) -> Result<SavedPlaylistSummary, String> {
    let db = Arc::clone(&state.db);
    let ids: Vec<i64> = state
        .playlist
        .snapshot()
        .playlist
        .iter()
        .filter_map(|item| item.as_track().map(|t| t.id))
        .collect();
    blocking(move || {
        let made = db
            .create_saved_playlist(&name, &ids, now_ms())
            .map_err(err)?;
        saved_playlists::emit(&handle, &db);
        Ok(made)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn saved_playlist_list(
    state: State<'_, AppState>,
) -> Result<Vec<SavedPlaylistSummary>, String> {
    let db = Arc::clone(&state.db);
    blocking(move || db.saved_playlists().map_err(err)).await
}

#[tauri::command(rename_all = "camelCase")]
async fn saved_playlist_get(
    state: State<'_, AppState>,
    id: i64,
) -> Result<Option<SavedPlaylist>, String> {
    let db = Arc::clone(&state.db);
    blocking(move || db.saved_playlist(id).map_err(err)).await
}

/// Create a saved playlist with its entries. One call, so a guest who may not
/// edit an existing list can still make a whole one.
#[tauri::command(rename_all = "camelCase")]
async fn saved_playlist_create(
    handle: AppHandle,
    state: State<'_, AppState>,
    name: String,
    track_ids: Vec<i64>,
) -> Result<SavedPlaylistSummary, String> {
    let db = Arc::clone(&state.db);
    blocking(move || {
        let made = db
            .create_saved_playlist(&name, &track_ids, now_ms())
            .map_err(err)?;
        saved_playlists::emit(&handle, &db);
        Ok(made)
    })
    .await
}

/// Write a saved playlist to `path` as a file another install can import.
#[tauri::command(rename_all = "camelCase")]
async fn saved_playlist_export(
    state: State<'_, AppState>,
    id: i64,
    path: String,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    blocking(move || {
        let file = db
            .export_saved_playlist(id)
            .map_err(err)?
            .ok_or_else(|| format!("no saved playlist {id}"))?;
        let json = serde_json::to_string_pretty(&file).map_err(err)?;
        std::fs::write(&path, json).map_err(|e| format!("could not write {path}: {e}"))
    })
    .await
}

/// Make a new saved playlist from the file at `path`. Never overwrites one.
#[tauri::command(rename_all = "camelCase")]
async fn saved_playlist_import(
    handle: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<SavedPlaylistSummary, String> {
    let db = Arc::clone(&state.db);
    blocking(move || {
        let json =
            std::fs::read_to_string(&path).map_err(|e| format!("could not read {path}: {e}"))?;
        let file = SavedPlaylistFile::parse(&json).map_err(err)?;
        let made = db.import_saved_playlist(&file, now_ms()).map_err(err)?;
        saved_playlists::emit(&handle, &db);
        Ok(made)
    })
    .await
}

/// Add entries for `track_ids`, in order, ahead of the entry at `index`. Absent
/// or `null` appends, as does an index past the end.
#[tauri::command(rename_all = "camelCase")]
async fn saved_playlist_add_entries(
    handle: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    track_ids: Vec<i64>,
    index: Option<usize>,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    blocking(move || {
        db.add_saved_entries(id, &track_ids, index, now_ms())
            .map_err(err)?;
        saved_playlists::emit(&handle, &db);
        Ok(())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn saved_playlist_remove_entry(
    handle: AppHandle,
    state: State<'_, AppState>,
    entry_id: i64,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    blocking(move || {
        db.remove_saved_entry(entry_id, now_ms()).map_err(err)?;
        saved_playlists::emit(&handle, &db);
        Ok(())
    })
    .await
}

/// Bind an entry to a track the operator picked for it.
#[tauri::command(rename_all = "camelCase")]
async fn saved_playlist_bind_entry(
    handle: AppHandle,
    state: State<'_, AppState>,
    entry_id: i64,
    track_id: i64,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    blocking(move || {
        db.bind_saved_entry(entry_id, track_id, now_ms())
            .map_err(err)?;
        saved_playlists::emit(&handle, &db);
        Ok(())
    })
    .await
}

/// Remove several entries of a saved playlist as one change.
#[tauri::command(rename_all = "camelCase")]
async fn saved_playlist_remove_entries(
    handle: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    entry_ids: Vec<i64>,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    blocking(move || {
        db.remove_saved_entries(id, &entry_ids, now_ms())
            .map_err(err)?;
        saved_playlists::emit(&handle, &db);
        Ok(())
    })
    .await
}

/// Move several entries as one block, in the order given, into the gap at
/// `index` of the saved playlist as it stands.
#[tauri::command(rename_all = "camelCase")]
async fn saved_playlist_move_entries(
    handle: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    entry_ids: Vec<i64>,
    index: usize,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    blocking(move || {
        db.move_saved_entries(id, &entry_ids, index, now_ms())
            .map_err(err)?;
        saved_playlists::emit(&handle, &db);
        Ok(())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn saved_playlist_move_entry(
    handle: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    from: usize,
    to: usize,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    blocking(move || {
        db.move_saved_entry(id, from, to, now_ms()).map_err(err)?;
        saved_playlists::emit(&handle, &db);
        Ok(())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn saved_playlist_rename(
    handle: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    name: String,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    blocking(move || {
        db.rename_saved_playlist(id, &name, now_ms()).map_err(err)?;
        saved_playlists::emit(&handle, &db);
        Ok(())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn saved_playlist_delete(
    handle: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    blocking(move || {
        db.delete_saved_playlist(id).map_err(err)?;
        saved_playlists::emit(&handle, &db);
        Ok(())
    })
    .await
}

/// Set or clear a queued item's override. `null` drops the item back to the
/// track's radio edit; an all-`null` object is a deliberate "whole file this
/// once" and is stored as one.
#[tauri::command(rename_all = "camelCase")]
fn playlist_set_item_cue_points(
    app: State<'_, AppState>,
    index: usize,
    cue_points: Option<CuePoints>,
) {
    app.playlist.set_item_cue_points(index, cue_points);
}

#[tauri::command(rename_all = "camelCase")]
fn playlist_add_stop_marker(app: State<'_, AppState>) {
    app.playlist.add_stop();
}

#[tauri::command(rename_all = "camelCase")]
fn playlist_add_filler(app: State<'_, AppState>, content_type: playlist::ContentType) {
    app.playlist.add_filler(content_type);
}

#[tauri::command(rename_all = "camelCase")]
fn playlist_remove(app: State<'_, AppState>, index: usize) {
    app.playlist.remove(index);
}

#[tauri::command(rename_all = "camelCase")]
fn playlist_move(app: State<'_, AppState>, from: usize, to: usize) {
    app.playlist.move_item(from, to);
}

#[tauri::command(rename_all = "camelCase")]
fn playlist_clear(app: State<'_, AppState>) {
    app.playlist.clear();
}

#[tauri::command(rename_all = "camelCase")]
fn playlist_play_index(app: State<'_, AppState>, index: usize) {
    app.playlist.play_index(index);
}

#[tauri::command(rename_all = "camelCase")]
fn playlist_play_now(app: State<'_, AppState>, id: i64) {
    app.playlist.play_now(id);
}

#[tauri::command(rename_all = "camelCase")]
fn playlist_next(app: State<'_, AppState>) {
    app.playlist.next();
}

/// Step back to the last track that aired.
#[tauri::command(rename_all = "camelCase")]
fn playlist_prev(app: State<'_, AppState>) {
    app.playlist.prev();
}

#[tauri::command(rename_all = "camelCase")]
fn playlist_stop(app: State<'_, AppState>) {
    app.playlist.stop();
}

#[tauri::command(rename_all = "camelCase")]
fn playlist_set_auto_playlist(app: State<'_, AppState>, active: bool) {
    app.playlist.set_auto_playlist(active);
}

/// Choose the auto-playlist source: a saved playlist, or `null` for the music
/// library. Refused for one with no playable music, which could only add
/// nothing. See `docs/saved-playlists.md`.
#[tauri::command(rename_all = "camelCase")]
async fn playlist_set_source(state: State<'_, AppState>, id: Option<i64>) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    let playlist = Arc::clone(&state.playlist);
    blocking(move || {
        let Some(id) = id else {
            playlist.set_source(None);
            return Ok(());
        };
        let (name, tracks) = db
            .saved_playlist_pool(id)
            .map_err(err)?
            .ok_or_else(|| format!("no saved playlist {id}"))?;
        if tracks == 0 {
            return Err(format!("\u{201c}{name}\u{201d} has no playable music"));
        }
        playlist.set_source(Some((id, name)));
        Ok(())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
fn playlist_set_auto_advance(app: State<'_, AppState>, active: bool) {
    app.playlist.set_auto_advance(active);
}

/// Whether the main deck is playing right now.
///
/// The renderer reads this at startup because `pause-state` is an event, and an
/// event emitted before the window was listening is gone: a session restored
/// during backend setup, or a reload mid-show, would otherwise leave the
/// transport button contradicting what is audible.
#[tauri::command(rename_all = "camelCase")]
fn main_deck_is_playing(app_state: State<'_, AppState>) -> bool {
    app_state.bus.main_is_playing()
}

#[tauri::command(rename_all = "camelCase")]
fn main_deck_play(app_state: State<'_, AppState>) {
    app_state.bus.send_main(Cmd::Play);
}

#[tauri::command(rename_all = "camelCase")]
fn main_deck_pause(app_state: State<'_, AppState>) {
    app_state.bus.send_main(Cmd::Pause);
}

#[tauri::command(rename_all = "camelCase")]
fn main_deck_stop(app_state: State<'_, AppState>) {
    app_state.bus.send_main(Cmd::Stop);
}

#[tauri::command(rename_all = "camelCase")]
fn main_deck_seek(app_state: State<'_, AppState>, seconds: f64) {
    app_state.bus.send_main(Cmd::Seek(seconds));
}

#[tauri::command(rename_all = "camelCase")]
fn main_deck_set_volume(app_state: State<'_, AppState>, volume: f32) {
    app_state.bus.send_main(Cmd::SetVolume(volume));
}

/// Ramp the on-air deck to silence, then stop it. The playlist stays where it
/// is, exactly as it does for Stop.
///
/// The duration comes from the stored config rather than the player tuning the
/// bus captured at spawn, so a change in Settings applies to the next press.
#[tauri::command(rename_all = "camelCase")]
fn main_deck_fade_out(app_state: State<'_, AppState>, ms: Option<u64>) {
    let ms = ms.unwrap_or_else(|| app_state.config.get_tuning().player.fade_out_ms);
    app_state.bus.send_main(Cmd::Fade {
        to: 0.0,
        ms,
        on_complete: Some(RampDone::Stop),
    });
}

/// Start the next item now and fade the outgoing track out underneath it.
///
/// With a deck armed and decoded this is a handover fired early: the same role
/// swap the bus performs at `next_start`, so the playlist engine reconciles
/// against `program:handover` with no special case.
///
/// With nothing armed there is nothing to overlap with, so the bus degrades it
/// to a fade that *ends* the track and the playlist advances under the rules it
/// already applies at the end of any track. That choice is the worker's because
/// only it knows what is decoded on which deck this tick.
#[tauri::command(rename_all = "camelCase")]
fn main_deck_fade_to_next(app_state: State<'_, AppState>, ms: Option<u64>) {
    let ms = ms.unwrap_or_else(|| app_state.config.get_tuning().player.fade_to_next_ms);
    app_state.bus.send_main(Cmd::HandOverNow { fade_ms: ms });
}

/// Return a track's stored amplitude-curve peaks (one byte per bucket) for the
/// seek UI, or `None` when the track has no waveform. Deck-agnostic — both the
/// main and cue decks render the same per-track curve.
///
/// Off the main thread: the renderer asks for this the instant a track change
/// lands, and the library's lock is shared with the analysis pass. On
/// `spawn_blocking`, not in the command body, so the wait for that lock is not a
/// held async runtime worker.
#[tauri::command(rename_all = "camelCase")]
async fn get_waveform(app_state: State<'_, AppState>, id: i64) -> Result<Option<Vec<u8>>, String> {
    let db = Arc::clone(&app_state.db);
    tauri::async_runtime::spawn_blocking(move || db.get_waveform(id))
        .await
        .map_err(err)?
        .map_err(err)
}

/// Decode a track into the cue editor's fine curve (see
/// [`audio_measure::waveform::compute_detail`]) and return it as raw bytes. The
/// file is taken from the prefetch cache when resident, so a networked share is
/// not read a second time for a track already queued.
#[tauri::command(rename_all = "camelCase")]
async fn get_waveform_detail(
    state: State<'_, AppState>,
    id: i64,
) -> Result<tauri::ipc::Response, String> {
    let db = Arc::clone(&state.db);
    let cache = Arc::clone(&state.cache);
    let detail = tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
        let bytes = match cache.get(id) {
            Some(b) => b,
            None => {
                let media = db
                    .get_media_track(id)?
                    .ok_or_else(|| anyhow::anyhow!("track not found"))?;
                Arc::from(std::fs::read(&media.path)?.into_boxed_slice())
            }
        };
        audio_measure::waveform::compute_detail(bytes)
    })
    .await
    .map_err(err)?
    .map_err(|e| format!("{e:#}"))?;
    Ok(tauri::ipc::Response::new(detail))
}

/// Extract a track's embedded cover art as a base64 `data:` URL for the deck's
/// vinyl disc, or `None` when the file has no artwork. Read on demand (like the
/// waveform) rather than stored, so the library DB stays free of image blobs.
///
/// Off the main thread, and not optional: this opens and parses the audio file
/// itself, which on a network share is a read of unbounded duration — on the
/// thread the window is drawn from, a slow share would freeze the UI for as long
/// as the mount takes to answer. The decks never read a share on their hot path
/// for the same reason; artwork must not be the exception.
///
/// On `spawn_blocking` rather than in the command body, because the duration is
/// unbounded: the renderer fires one of these per track change, and a handful of
/// skips over a wedged share would otherwise park every async runtime worker and
/// take the rest of the async commands down with them.
#[tauri::command(rename_all = "camelCase")]
async fn get_cover_art(app_state: State<'_, AppState>, id: i64) -> Result<Option<String>, String> {
    let db = Arc::clone(&app_state.db);
    tauri::async_runtime::spawn_blocking(move || -> Result<Option<String>, String> {
        let media = db.get_media_track(id).map_err(err)?;
        Ok(media.and_then(|m| library::scanner::read_cover_art(&m.path)))
    })
    .await
    .map_err(err)?
}

/// Enumerating outputs goes out to the host audio API, which a wedged device can
/// take its time answering.
#[tauri::command(rename_all = "camelCase")]
async fn audio_list_devices() -> Result<Vec<DeviceInfo>, String> {
    blocking(move || Ok(list_output_devices())).await
}

#[tauri::command(rename_all = "camelCase")]
async fn update_track_metadata(
    app: AppHandle,
    state: State<'_, AppState>,
    updates: TrackMetadataUpdate,
) -> Result<Track, String> {
    let db = Arc::clone(&state.db);
    let config = Arc::clone(&state.config);
    let health = Arc::clone(&state.health);
    let tag_writer = Arc::clone(&state.tag_writer);
    let waveform = Arc::clone(&state.waveform);
    // Writing tags to the file and re-deriving cue points are the owner's.
    let owns_files = !state.hub.is_studio();
    // So is which library a track belongs to: it follows the library path,
    // and the next scan there would put it back.
    let updates = TrackMetadataUpdate {
        content_type: updates.content_type.filter(|_| owns_files),
        ..updates
    };
    blocking(move || {
        // Read before the write, so the kick below fires on a class that
        // actually moved rather than on every save that carries the field.
        let reclassified = match &updates.content_type {
            Some(next) => db
                .track_content_type(updates.id)
                .map_err(err)?
                .is_some_and(|was| was != *next),
            None => false,
        };
        let track = db.update_track_metadata(&updates).map_err(err)?;
        if track.edited_fields != 0 && owns_files {
            tag_writer.request(track.id);
        }
        // The update itself requeued the track: a reclassification changes what
        // automatic analysis would infer. This kicks the pass that drains the
        // queue.
        if reclassified && owns_files {
            waveform.start(app, Arc::clone(&db), config);
        }
        // Artist and title decide possible duplicates.
        health.refresh();
        Ok(track)
    })
    .await
}

/// Drop a track's metadata edits and take its tags from the file again. The
/// file is re-read now: a rescan skips a file whose mtime has not changed.
#[tauri::command(rename_all = "camelCase")]
async fn revert_track_tags(state: State<'_, AppState>, id: i64) -> Result<Track, String> {
    let db = Arc::clone(&state.db);
    let track = tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<Track> {
        let media = db
            .get_media_track(id)?
            .ok_or_else(|| anyhow::anyhow!("track not found"))?;
        let parsed = library::scanner::read_file_tags(&media.path)?;
        db.revert_track_tags(id, &parsed)
    })
    .await
    .map_err(err)?
    .map_err(|e| format!("{e:#}"))?;
    state.tag_writer.dismiss(id);
    state.health.refresh();
    Ok(track)
}

/// Try a failed tag write again.
#[tauri::command(rename_all = "camelCase")]
fn retry_tag_write(state: State<'_, AppState>, id: i64) {
    state.tag_writer.retry(id);
}

/// Stop listing a failed tag write. The edit stays in the library.
///
/// Off the main thread because dropping a failure notifies the health listener,
/// and that rebuilds the whole report.
#[tauri::command(rename_all = "camelCase")]
async fn dismiss_tag_write(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let tag_writer = Arc::clone(&state.tag_writer);
    blocking(move || {
        tag_writer.dismiss(id);
        Ok(())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
fn get_main_device(state: State<'_, AppState>) -> Option<DeviceRef> {
    state.config.get_main_device()
}

#[tauri::command(rename_all = "camelCase")]
async fn set_main_device(
    state: State<'_, AppState>,
    device: Option<DeviceRef>,
) -> Result<(), String> {
    let config = Arc::clone(&state.config);
    blocking(move || {
        config.set_main_device(device).map_err(err)?;
        log::info!("main device updated; restart required to apply");
        Ok(())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
fn get_cue_device(state: State<'_, AppState>) -> Option<DeviceRef> {
    state.config.get_cue_device()
}

#[tauri::command(rename_all = "camelCase")]
async fn set_cue_device(
    state: State<'_, AppState>,
    device: Option<DeviceRef>,
) -> Result<(), String> {
    let config = Arc::clone(&state.config);
    let cue = Arc::clone(&state.cue);
    blocking(move || {
        config.set_cue_device(device).map_err(err)?;
        // Invalidate cached cue handle so the next cue_* command spawns
        // against the new device. Dropping the Sender stops the worker thread.
        *cue.lock() = None;
        Ok(())
    })
    .await
}

/// Ensure a `CueDeck` exists for the configured cue device.
/// Lazy-spawned on first cue command. Errors if no cue device is set
/// or the saved device cannot be resolved.
fn with_cue<F>(state: &State<'_, AppState>, f: F) -> Result<(), String>
where
    F: FnOnce(&CueDeck),
{
    let mut guard = state.cue.lock();
    if guard.is_none() {
        let cue_ref = state
            .config
            .get_cue_device()
            .ok_or_else(|| "no cue device configured; pick one in Settings → Audio".to_string())?;
        // The worker resolves the DeviceRef lazily and falls back to the default
        // output, mirroring the main deck's self-healing open (#259).
        *guard = Some(CueDeck::spawn(
            state.app_handle.clone(),
            Some(cue_ref),
            Arc::clone(&state.cache),
            player_tuning_from(&state.config.get_tuning()),
        ));
    }
    if let Some(handle) = guard.as_ref() {
        f(handle);
    }
    Ok(())
}

/// Load a track onto the cue deck.
///
/// `cuePoints` picks the audition mode. Absent (*Absolute*) plays the whole
/// file with no markers applied, so an operator can scrub it to find an
/// in-point. Present (*Preview*) applies exactly those markers — the editor
/// sends its unsaved draft, so a ramp can be heard before it is committed.
///
/// `autoplay` travels with the load rather than arriving as a later `Play`,
/// because the load completes on a background thread and parks the sink when
/// it lands — a `Play` sent in between would be undone by it.
///
/// The only `cue_*` command that is not a bare channel send: it resolves the
/// track first, which is a read of the one `Mutex<Connection>` the analysis
/// pass also writes through. Inline, that put the window's thread behind a
/// database lock every time the operator auditioned a track.
#[tauri::command(rename_all = "camelCase")]
async fn cue_load(
    state: State<'_, AppState>,
    id: i64,
    cue_points: Option<CuePoints>,
    autoplay: Option<bool>,
    start_at: Option<f64>,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    let config = Arc::clone(&state.config);
    let (track, gain) = blocking(move || {
        let track = db
            .get_media_track(id)
            .map_err(err)?
            .ok_or_else(|| "track not found".to_string())?;
        let gain = audio::levelling::factor(
            config.get_tuning().player.replay_gain,
            track.loudness.gain_db,
            track.loudness.peak,
        );
        Ok((track, gain))
    })
    .await?;
    with_cue(&state, |h| {
        h.send(Cmd::Load {
            id,
            path: std::path::PathBuf::from(track.path),
            duration: if track.duration > 0.0 {
                Some(track.duration)
            } else {
                None
            },
            cue_points: cue_points.unwrap_or_default(),
            // Air seconds. The cue editor reloads an edited audition where it
            // was; the worker clamps a start past the new air duration.
            start_at: start_at.unwrap_or(0.0),
            // The cue deck is off the program bus; nothing it does is air.
            bound_dead_air: false,
            // Parked by default. Cueing a track is a staging action — the
            // operator decides when it makes noise, and switching audition
            // mode reloads the deck, so autoplay would restart the audio on
            // every Absolute/Preview toggle. The cue editor's transport is the
            // explicit ask, and sets this.
            autoplay: autoplay.unwrap_or(false),
            gain,
        });
    })
}

#[tauri::command(rename_all = "camelCase")]
fn cue_play(state: State<'_, AppState>) -> Result<(), String> {
    with_cue(&state, |h| h.send(Cmd::Play))
}

#[tauri::command(rename_all = "camelCase")]
fn cue_pause(state: State<'_, AppState>) -> Result<(), String> {
    with_cue(&state, |h| h.send(Cmd::Pause))
}

#[tauri::command(rename_all = "camelCase")]
fn cue_stop(state: State<'_, AppState>) -> Result<(), String> {
    with_cue(&state, |h| h.send(Cmd::Stop))
}

#[tauri::command(rename_all = "camelCase")]
fn cue_seek(state: State<'_, AppState>, seconds: f64) -> Result<(), String> {
    with_cue(&state, |h| h.send(Cmd::Seek(seconds)))
}

#[tauri::command(rename_all = "camelCase")]
fn cue_set_volume(state: State<'_, AppState>, volume: f32) -> Result<(), String> {
    with_cue(&state, |h| h.send(Cmd::SetVolume(volume)))
}

/// What the Settings page shows of the shared library: what `config.json`
/// asks for, and where the role this launch took up stands.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SharedLibrary {
    role: LibraryRole,
    url: Option<String>,
    machine_name: Option<String>,
    allow_unencrypted: bool,
    direct_tls: bool,
    ca_certificate: Option<CaCertificate>,
    status: hub::Status,
}

/// A certificate authority for the hub: its PEM text, which the page sends
/// back when it saves, and what it is.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CaCertificate {
    pem: String,
    summary: hub::CaSummary,
}

fn shared_library(state: &AppState) -> SharedLibrary {
    let saved = state.config.external_library();
    SharedLibrary {
        role: saved.role,
        url: saved.url,
        machine_name: saved.machine_name,
        allow_unencrypted: saved.allow_unencrypted,
        direct_tls: saved.direct_tls,
        // One that no longer reads is dropped from the page, not shown as
        // trusted: the worker refuses to start on it and says so.
        ca_certificate: saved.ca_certificate.and_then(|pem| {
            let summary = hub::summarize_ca(&pem).ok()?;
            Some(CaCertificate { pem, summary })
        }),
        status: state.hub.status(),
    }
}

/// Read a certificate file the operator picked as the hub's authority. Nothing
/// is stored: the page shows what it is and sends the text back on save.
#[tauri::command(rename_all = "camelCase")]
async fn read_ca_certificate(path: String) -> Result<CaCertificate, String> {
    blocking(move || {
        let size = std::fs::metadata(&path).map_err(err)?.len();
        if size > hub::MAX_PEM_BYTES {
            return Err("this file is too large to be a certificate".to_owned());
        }
        let pem = std::fs::read_to_string(&path)
            .map_err(|_| "this file is not a PEM certificate".to_owned())?;
        let summary = hub::summarize_ca(&pem).map_err(err)?;
        Ok(CaCertificate { pem, summary })
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
fn get_shared_library(state: State<'_, AppState>) -> SharedLibrary {
    shared_library(&state)
}

/// Save the role, the hub's address and this machine's name. None of it takes
/// effect before the next launch: a role is not changed under a running
/// playlist.
#[tauri::command(rename_all = "camelCase")]
async fn set_shared_library(
    state: State<'_, AppState>,
    settings: SharedLibrarySettings,
) -> Result<SharedLibrary, String> {
    let config = Arc::clone(&state.config);
    blocking(move || {
        if let Some(pem) = &settings.ca_certificate {
            hub::summarize_ca(pem).map_err(err)?;
        }
        config.set_external_library(settings).map_err(err)
    })
    .await?;
    Ok(shared_library(&state))
}

#[tauri::command(rename_all = "camelCase")]
fn get_now_playing_config(state: State<'_, AppState>) -> NowPlayingConfig {
    state.config.get_now_playing()
}

#[tauri::command(rename_all = "camelCase")]
async fn set_now_playing_config(
    state: State<'_, AppState>,
    config: NowPlayingConfig,
) -> Result<(), String> {
    let store = Arc::clone(&state.config);
    blocking(move || store.set_now_playing(config).map_err(err)).await
}

#[tauri::command(rename_all = "camelCase")]
fn get_tuning_config(state: State<'_, AppState>) -> TuningConfig {
    state.config.get_tuning()
}

/// Persist new tuning values and return the clamped result the UI should
/// display. Interleave/auto-playlist changes apply live; cache/player changes
/// (which are captured by long-lived worker threads at startup) take effect on
/// the next restart — the UI surfaces that hint.
/// Store a track's cue points. Returns the clamped value the backend actually
/// persisted, so the UI adopts the one rule rather than reimplementing it.
#[tauri::command(rename_all = "camelCase")]
async fn set_cue_points(
    state: State<'_, AppState>,
    id: i64,
    points: CuePoints,
) -> Result<CuePoints, String> {
    let db = Arc::clone(&state.db);
    let playlist = Arc::clone(&state.playlist);
    blocking(move || {
        let stored = db.set_cue_points(id, points).map_err(err)?;
        // Queued items hold a copy of the track for display. Refresh it, or the
        // next snapshot would undo the renderer's optimistic patch and put the
        // pre-edit duration back on the row.
        playlist.on_cue_points_saved(id, stored);
        Ok(stored)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn set_tuning_config(
    state: State<'_, AppState>,
    config: TuningConfig,
) -> Result<TuningConfig, String> {
    let store = Arc::clone(&state.config);
    let db = Arc::clone(&state.db);
    let playlist = Arc::clone(&state.playlist);
    blocking(move || {
        let was = store.get_tuning().auto_cue;
        let stored = store.set_tuning(config).map_err(err)?;
        // Switching the automatic cue points on or off — either the whole trio
        // or the Next Start alone — changes what every derived set means, so
        // the library's answer changes and the copies the playlist holds have
        // to be re-read. Analysis is untouched: it runs and stores its results
        // either way, which is what makes both switches instant both ways.
        let now = stored.auto_cue;
        if (now.apply, now.apply_next_start) != (was.apply, was.apply_next_start) {
            db.set_auto_cue_policy(now.apply, now.apply_next_start);
            playlist.reload_cue_points();
        }
        Ok(stored)
    })
    .await
}

/// The resolved appearance the renderer paints. Ungated by necessity: the
/// renderer asks for this before it mounts, on a launch that starts locked.
#[tauri::command(rename_all = "camelCase")]
async fn get_appearance(state: State<'_, AppState>) -> Result<Appearance, String> {
    let theme = Theme::of(&state);
    blocking(move || Ok(theme.resolve())).await
}

/// Every theme the operator can pick, plus the ones that failed to load and
/// why. Ungated for the same reason as `get_appearance`.
#[tauri::command(rename_all = "camelCase")]
async fn list_themes(state: State<'_, AppState>) -> Result<Vec<ThemeListing>, String> {
    let data_dir = state.data_dir.clone();
    blocking(move || Ok(appearance::store::list(&data_dir))).await
}

#[tauri::command(rename_all = "camelCase")]
async fn set_theme(state: State<'_, AppState>, theme_id: String) -> Result<Appearance, String> {
    let theme = Theme::of(&state);
    blocking(move || {
        theme.update(|appearance| appearance.theme_id = theme_id)?;
        Ok(theme.resolve())
    })
    .await
}

/// Name the station. Trimmed and capped by the config layer, which returns what
/// it stored; a blank name means the product name is used.
#[tauri::command(rename_all = "camelCase")]
async fn set_station_name(
    state: State<'_, AppState>,
    name: Option<String>,
) -> Result<Appearance, String> {
    let theme = Theme::of(&state);
    blocking(move || {
        theme.update(|appearance| appearance.station_name = name)?;
        Ok(theme.resolve())
    })
    .await
}

/// Adopt an operator-chosen image into `{app_data_dir}/branding` and point the
/// slot at it. Only the file name is stored, never the path it came from.
#[tauri::command(rename_all = "camelCase")]
async fn set_station_image(
    state: State<'_, AppState>,
    slot: appearance::ImageSlot,
    path: String,
) -> Result<Appearance, String> {
    let theme = Theme::of(&state);
    blocking(move || {
        let name = appearance::adopt_image(&theme.data_dir, slot, std::path::Path::new(&path))?;
        theme.update(|appearance| match slot {
            appearance::ImageSlot::Logo => appearance.logo = Some(name),
            appearance::ImageSlot::Label => appearance.label = Some(name),
        })?;
        Ok(theme.resolve())
    })
    .await
}

/// Clear a slot, so the theme's image (or the bundled default) shows again. The
/// file is left in `branding/` — deleting it buys nothing and loses an undo.
#[tauri::command(rename_all = "camelCase")]
async fn clear_station_image(
    state: State<'_, AppState>,
    slot: appearance::ImageSlot,
) -> Result<Appearance, String> {
    let theme = Theme::of(&state);
    blocking(move || {
        theme.update(|appearance| match slot {
            appearance::ImageSlot::Logo => appearance.logo = None,
            appearance::ImageSlot::Label => appearance.label = None,
        })?;
        Ok(theme.resolve())
    })
    .await
}

/// Whether the art on a deck's vinyl turns while the deck plays.
#[tauri::command(rename_all = "camelCase")]
async fn set_spin_vinyl(state: State<'_, AppState>, enabled: bool) -> Result<Appearance, String> {
    let theme = Theme::of(&state);
    blocking(move || {
        theme.update(|appearance| appearance.spin_vinyl = enabled)?;
        Ok(theme.resolve())
    })
    .await
}

/// Re-read the themes directory and re-resolve the active theme, so an edit to
/// the theme on screen takes effect. A repaint is always an explicit ask —
/// there is no filesystem watcher.
#[tauri::command(rename_all = "camelCase")]
async fn reload_themes(state: State<'_, AppState>) -> Result<Appearance, String> {
    let theme = Theme::of(&state);
    blocking(move || Ok(theme.resolve())).await
}

/// Open the themes directory in the operator's file manager. Takes no argument
/// and opens a directory the app owns, so it is narrower than `reveal_track`.
#[tauri::command(rename_all = "camelCase")]
async fn reveal_themes_dir(state: State<'_, AppState>) -> Result<(), String> {
    let data_dir = state.data_dir.clone();
    blocking(move || {
        let dir = appearance::store::themes_dir(&data_dir);
        appearance::store::seed_if_absent(&data_dir).map_err(err)?;
        tauri_plugin_opener::reveal_item_in_dir(&dir).map_err(err)
    })
    .await
}

/// What the appearance commands need once they are off the main thread: the
/// stored configuration and the directory the themes and branding images live
/// in. Cloned out of [`AppState`] before the await, since the state guard
/// cannot cross one.
struct Theme {
    config: Arc<Config>,
    data_dir: std::path::PathBuf,
}

impl Theme {
    fn of(state: &State<'_, AppState>) -> Self {
        Self {
            config: Arc::clone(&state.config),
            data_dir: state.data_dir.clone(),
        }
    }

    /// Resolve the configured theme into what the renderer paints. Never fails:
    /// a theme that cannot be loaded falls back to Midnight and says so.
    fn resolve(&self) -> Appearance {
        appearance::resolve(&self.data_dir, &self.config.get_appearance())
    }

    /// Read, change and store the appearance configuration.
    fn update(&self, change: impl FnOnce(&mut AppearanceConfig)) -> Result<(), String> {
        let mut config = self.config.get_appearance();
        change(&mut config);
        self.config.set_appearance(config).map_err(err)?;
        Ok(())
    }
}

#[tauri::command(rename_all = "camelCase")]
async fn now_playing_test(state: State<'_, AppState>) -> Result<u16, String> {
    let broadcast = Arc::clone(&state.broadcast);
    blocking(move || broadcast.test_webhook_blocking()).await
}

#[tauri::command(rename_all = "camelCase")]
async fn broadcast_shutdown(state: State<'_, AppState>) -> Result<(), String> {
    let broadcast = Arc::clone(&state.broadcast);
    blocking(move || {
        broadcast.shutdown_blocking();
        Ok(())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
fn scan_libraries(app: AppHandle, state: State<'_, AppState>) -> StartResult {
    // A scan is the operator asking for the library to be brought up to date,
    // which includes any row a cancelled backfill left behind: `cancel` is only
    // cleared by `start`, and launch is otherwise the one place that calls it.
    Arc::clone(&state.tag_backfill).start(app.clone(), Arc::clone(&state.db));
    Arc::clone(&state.scan).start(
        app,
        Arc::clone(&state.db),
        Arc::clone(&state.config),
        Arc::clone(&state.waveform),
    )
}

#[tauri::command(rename_all = "camelCase")]
fn cancel_scan(state: State<'_, AppState>) {
    state.scan.cancel();
    state.waveform.cancel();
    state.tag_backfill.cancel();
}

/// Stop the analysis pass on its own, for when it is running without a scan —
/// every launch, and after any scan that finished. Cancelled rows stay
/// `pending`, and the cancel dies with the pass, so the next scan, launch or
/// reclassification picks them up again.
#[tauri::command(rename_all = "camelCase")]
fn cancel_analysis(state: State<'_, AppState>) {
    state.waveform.cancel();
}

/// Permanently delete the chosen missing tracks. Ids of tracks that are not
/// missing are ignored. Refused mid-scan: the scan may be about to reattach
/// some of them. Returns how many were deleted.
///
/// On `spawn_blocking` for the delete *and* the refresh: `Health::refresh`
/// rebuilds the whole report and emits it inline, and the playlist listens to
/// that event with a full transition — on the main thread that is the very stall
/// the playlist's command thread exists to avoid.
#[tauri::command(rename_all = "camelCase")]
async fn purge_tracks(state: State<'_, AppState>, ids: Vec<i64>) -> Result<usize, String> {
    if state.scan.is_running() {
        return Err("a library scan is running; purge when it finishes".into());
    }
    let db = Arc::clone(&state.db);
    let health = Arc::clone(&state.health);
    let playlist = Arc::clone(&state.playlist);
    tauri::async_runtime::spawn_blocking(move || {
        let deleted = db.purge_tracks(&ids).map_err(err)?;
        // No queued item may point at a row that no longer exists.
        playlist.remove_tracks(deleted.iter().copied().collect());
        health.refresh();
        Ok(deleted.len())
    })
    .await
    .map_err(err)?
}

/// Hide the chosen tracks from the library: out of every tab, search, count,
/// duplicate finding and auto-playlist pick, and unplayable where they are
/// already queued. Nothing is deleted. Ids of tracks that are missing or
/// already hidden are ignored. Returns how many were hidden.
///
/// The health refresh is what tells the playlist, so it runs on
/// `spawn_blocking` for the reason `purge_tracks` gives.
#[tauri::command(rename_all = "camelCase")]
async fn hide_tracks(state: State<'_, AppState>, ids: Vec<i64>) -> Result<usize, String> {
    let db = Arc::clone(&state.db);
    let health = Arc::clone(&state.health);
    tauri::async_runtime::spawn_blocking(move || {
        let hidden = db.hide_tracks(&ids, now_ms()).map_err(err)?;
        health.refresh();
        Ok(hidden.len())
    })
    .await
    .map_err(err)?
}

/// Put the chosen hidden tracks back in the library. Returns how many were
/// restored.
#[tauri::command(rename_all = "camelCase")]
async fn unhide_tracks(state: State<'_, AppState>, ids: Vec<i64>) -> Result<usize, String> {
    let db = Arc::clone(&state.db);
    let health = Arc::clone(&state.health);
    tauri::async_runtime::spawn_blocking(move || {
        let restored = db.unhide_tracks(&ids).map_err(err)?;
        health.refresh();
        Ok(restored.len())
    })
    .await
    .map_err(err)?
}

/// Apply the current automatic-analysis thresholds to material already in the
/// library. Changing a threshold never re-analyses anything by itself, so this
/// is how an operator makes a new one reach what they already have.
///
/// Rows carrying a readable level envelope are re-derived on the spot, with no
/// decode. The rest go back to the analysis pass, kicked once here — `start`
/// rather than `nudge`, so a pass stopped earlier does not swallow it. Radio
/// edits the operator made are left alone and only counted.
///
/// Refused mid-scan, like a purge: a scan rewrites these rows underneath.
///
/// The work runs on `spawn_blocking`, like every other command here that takes
/// more than a moment: it holds the DB mutex for a statement per row, and a
/// blocking call in the command body itself would hold an async runtime worker
/// for the duration.
#[tauri::command(rename_all = "camelCase")]
async fn recalculate_auto_cue(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Recalculated, String> {
    if state.scan.is_running() {
        return Err("a library scan is running; recalculate when it finishes".into());
    }
    let thresholds = state.config.get_tuning().auto_cue.thresholds();
    let db = Arc::clone(&state.db);
    let done = tauri::async_runtime::spawn_blocking(move || {
        db.recalculate_auto_cue(thresholds, library::scanner::now_ms())
    })
    .await
    .map_err(err)?
    .map_err(err)?;

    // Kicked whatever the count says. `queued` reports only the rows that left
    // a settled result, but a row already waiting may have had a recorded
    // decode failure cleared just now — and that is precisely the row the pass
    // could not see before. `start` is single-flight and drains to nothing when
    // there is no work, so being wrong here costs a thread that exits.
    Arc::clone(&state.waveform).start(app, Arc::clone(&state.db), Arc::clone(&state.config));

    // Every derived set the library reports has just changed, so the copies the
    // playlist holds are stale — the same re-read a switch flip runs.
    if done.updated > 0 {
        state.playlist.reload_cue_points();
    }
    // `health` is not refreshed here on purpose: the pass kicked above emits
    // `waveform-state-changed`, which `Health::attach_to_app` already listens
    // for, and a cleared `analysis_failed_at` is the only thing here the report
    // counts.
    Ok(done)
}

#[tauri::command(rename_all = "camelCase")]
fn library_health(state: State<'_, AppState>) -> HealthReport {
    state.health.report()
}

/// Compare the disk with the library now, outside the timer.
#[tauri::command(rename_all = "camelCase")]
fn library_check_now(state: State<'_, AppState>) {
    state.check.request();
}

#[tauri::command(rename_all = "camelCase")]
async fn health_dismiss(
    state: State<'_, AppState>,
    kind: FindingKind,
    key: String,
) -> Result<(), String> {
    let health = Arc::clone(&state.health);
    blocking(move || health.dismiss(kind, &key).map_err(err)).await
}

#[tauri::command(rename_all = "camelCase")]
async fn health_undismiss(
    state: State<'_, AppState>,
    kind: FindingKind,
    key: String,
) -> Result<(), String> {
    let health = Arc::clone(&state.health);
    blocking(move || health.undismiss(kind, &key).map_err(err)).await
}

#[tauri::command(rename_all = "camelCase")]
fn get_scan_status(state: State<'_, AppState>) -> ScanStatus {
    state.scan.status()
}

#[tauri::command(rename_all = "camelCase")]
fn get_waveform_status(state: State<'_, AppState>) -> WaveformStatus {
    state.waveform.status()
}

/// Wrap the command handler so admin-only commands are refused while admin
/// mode is locked. See `admin::ADMIN_COMMANDS`.
fn admin_gated<R: tauri::Runtime>(
    handler: impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static,
) -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
    move |invoke| {
        let command = invoke.message.command();
        let refused = invoke
            .message
            .webview()
            .try_state::<AppState>()
            .and_then(|state| {
                state
                    .admin
                    .gate(command)
                    .err()
                    .map(|e| e.to_string())
                    .or_else(|| state.hub.gate(command).err().map(str::to_owned))
            });
        if let Some(e) = refused {
            log::warn!("refused {}: {e}", invoke.message.command());
            invoke.resolver.reject(e);
            return true;
        }
        handler(invoke)
    }
}

#[tauri::command(rename_all = "camelCase")]
fn admin_status(state: State<'_, AppState>) -> AdminStatus {
    state.admin.status()
}

/// Returns whether the password matched.
#[tauri::command(rename_all = "camelCase")]
async fn admin_unlock(state: State<'_, AppState>, password: String) -> Result<bool, String> {
    let admin = Arc::clone(&state.admin);
    tauri::async_runtime::spawn_blocking(move || admin.unlock(&password))
        .await
        .map_err(err)
}

#[tauri::command(rename_all = "camelCase")]
fn admin_lock(state: State<'_, AppState>) {
    state.admin.lock();
}

#[tauri::command(rename_all = "camelCase")]
async fn admin_set_password(state: State<'_, AppState>, password: String) -> Result<(), String> {
    let admin = Arc::clone(&state.admin);
    tauri::async_runtime::spawn_blocking(move || admin.set_password(&password))
        .await
        .map_err(err)?
        .map_err(err)
}

#[tauri::command(rename_all = "camelCase")]
async fn admin_clear_password(state: State<'_, AppState>) -> Result<(), String> {
    let admin = Arc::clone(&state.admin);
    blocking(move || admin.clear_password().map_err(err)).await
}

#[tauri::command(rename_all = "camelCase")]
async fn admin_set_idle_lock_min(state: State<'_, AppState>, minutes: u64) -> Result<(), String> {
    let admin = Arc::clone(&state.admin);
    blocking(move || admin.set_idle_lock_min(minutes).map_err(err)).await
}

/// The updater's state: the running version, and what a check last found.
#[tauri::command(rename_all = "camelCase")]
fn update_status(state: State<AppState>) -> UpdateState {
    state.updater.status()
}

/// Ask whether a newer release exists. Open while admin mode is locked: it
/// reads a manifest and changes nothing.
#[tauri::command(rename_all = "camelCase")]
async fn update_check(state: State<'_, AppState>) -> Result<UpdateState, String> {
    let updater = Arc::clone(&state.updater);
    Ok(updater.check(false).await)
}

/// Install the offered release and restart. Resolves only on failure.
#[tauri::command(rename_all = "camelCase")]
async fn update_install(state: State<'_, AppState>) -> Result<(), String> {
    let updater = Arc::clone(&state.updater);
    updater.install().await
}

/// A page the app links out to. Named rather than passed as a URL, so the
/// renderer cannot open an address of its choosing.
#[derive(serde::Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
enum Link {
    Website,
    Changelog,
    Source,
    Licence,
    Notices,
}

impl Link {
    fn url(self) -> &'static str {
        match self {
            Self::Website => "https://radiodiodj.org",
            Self::Changelog => "https://github.com/Arskah/radiodiodj/blob/main/CHANGELOG.md",
            Self::Source => "https://github.com/Arskah/radiodiodj",
            Self::Licence => "https://github.com/Arskah/radiodiodj/blob/main/LICENSE",
            Self::Notices => {
                "https://github.com/Arskah/radiodiodj/blob/main/THIRD-PARTY-NOTICES.md"
            }
        }
    }
}

/// Open one of the project's pages in the default browser.
#[tauri::command(rename_all = "camelCase")]
async fn open_link(link: Link) -> Result<(), String> {
    blocking(move || tauri_plugin_opener::open_url(link.url(), None::<&str>).map_err(err)).await
}

/// Build the Tauri app and run it: plugins, then the data directory and the
/// library, then the decks, the playlist and the background jobs.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let log_level = std::env::var("RUST_LOG")
        .ok()
        .and_then(|s| s.parse::<log::LevelFilter>().ok())
        .unwrap_or(if cfg!(debug_assertions) {
            log::LevelFilter::Debug
        } else {
            log::LevelFilter::Info
        });

    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .targets([
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Webview),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                        file_name: None,
                    }),
                ])
                .level(log_level)
                .level_for("symphonia", log::LevelFilter::Warn)
                .level_for("symphonia_core", log::LevelFilter::Warn)
                .level_for("symphonia_bundle_mp3", log::LevelFilter::Error)
                // At debug it writes every statement with its parameters, which
                // for the hub is each track's whole document and waveform.
                .level_for("tokio_postgres", log::LevelFilter::Warn)
                .max_file_size(1024 * 1024)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepOne)
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::POSITION
                        | tauri_plugin_window_state::StateFlags::MAXIMIZED,
                )
                .build(),
        )
        .setup(|app| {
            std::panic::set_hook(Box::new(|info| {
                let bt = std::backtrace::Backtrace::force_capture();
                log::error!("panic: {}\n{}", info, bt);
            }));
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let config = Arc::new(Config::open(&data_dir)?);
            let role = hub::role_in_effect(&config);
            let studio = role == LibraryRole::Studio;
            let (opened, library_joined) =
                match Db::open_for(&data_dir.join("radiodiodj.db"), studio) {
                    Ok(opened) => opened,
                    Err(e) => match e.downcast_ref::<OpenError>() {
                        Some(refusal) => {
                            refuse_to_start(app.handle(), refusal);
                            return Ok(());
                        }
                        None => return Err(e.into()),
                    },
                };
            let library_reset = opened.reset_backup.is_some();
            let db = Arc::new(opened.db);
            // The library applies the automatic-cue policy on the way out, so
            // it has to know it before anything reads a track.
            let auto_cue = config.get_tuning().auto_cue;
            db.set_auto_cue_policy(auto_cue.apply, auto_cue.apply_next_start);
            // The folders in `config.json` are keyed by library path ids, and
            // after a join those ids are the owner's: path 1 there need not be
            // path 1 here. So a studio starts with every path unlocated.
            if library_joined {
                config.adopt_mounts(BTreeMap::new())?;
            }
            // Nor can it name a file until it knows where this machine keeps
            // each library path. A failure leaves the paths unlocated, which
            // plays nothing from them but loses nothing either.
            if let Err(e) = roots::adopt(&db, &config) {
                log::error!("library paths could not be read from the config: {e:#}");
            }

            // Give a first-run operator something to copy. Only when themes/ is
            // absent, so deleting the example does not bring it back.
            if let Err(e) = appearance::store::seed_if_absent(&data_dir) {
                log::warn!("could not seed the themes directory: {e}");
            }
            let admin = Arc::new(AdminLock::new(
                Arc::clone(&config),
                Some(app.handle().clone()),
            ));
            let session = Arc::new(Session::open(&data_dir));
            if library_reset || library_joined {
                session.forget_tracks()?;
            }
            // Pass the saved DeviceRef (not a pre-resolved device): the worker
            // resolves it lazily on the audio thread and falls back to the
            // system default, so a device unavailable at launch no longer kills
            // playback for the whole session (#259).
            let main_device = config.get_main_device();
            // The cache cap and player timeouts are read once here and captured
            // by their worker threads for the process lifetime; editing them
            // takes effect on the next launch.
            let tuning = config.get_tuning();
            let cache = Cache::new(app.handle().clone(), tuning.cache.max_cache_bytes);
            let bus = Arc::new(ProgramBus::spawn(
                app.handle().clone(),
                main_device,
                Arc::clone(&cache),
                player_tuning_from(&tuning),
            ));
            let broadcast = Arc::new(BroadcastService::new(
                Arc::clone(&config),
                default_now_playing_dir(&data_dir),
            )?);
            broadcast.attach_to_app(app.handle());
            // The playlist listens to the same deck topics the broadcast service
            // does, and drives advancement off them. Attach before hydrating so
            // the restored track's events are not missed.
            let playlist = Arc::new(PlaylistService::new(
                app.handle().clone(),
                Arc::clone(&db),
                Arc::clone(&config),
                Arc::clone(&bus),
                Arc::clone(&cache),
                Arc::clone(&broadcast),
            ));
            playlist.attach_to_app(app.handle());
            playlist.hydrate(&session.load());
            let waveform = Arc::new(WaveformJob::default());
            // Backfill waveforms, loudness and automatic cue points for any
            // already-indexed track that lacks one, without waiting for the
            // next scan. No-op on an empty library.
            // Not on a studio, here or below: decoding, reading tags, checking
            // the disk and scanning are the library owner's.
            if !studio {
                Arc::clone(&waveform).start(
                    app.handle().clone(),
                    Arc::clone(&db),
                    Arc::clone(&config),
                );
            }
            let tag_backfill = Arc::new(TagBackfillJob::default());
            // Fill tag columns added after a row was last read. No-op once
            // every row is at the current version, which is the steady state.
            if !studio {
                Arc::clone(&tag_backfill).start(app.handle().clone(), Arc::clone(&db));
            }
            let tag_writer = TagWriter::new(Arc::clone(&db), Arc::clone(&config));
            let health = Health::new(
                app.handle().clone(),
                Arc::clone(&db),
                Arc::clone(&tag_writer),
            );
            health.attach_to_app(app.handle());
            let scan = Arc::new(ScanState::default());
            let check = LibraryCheck::new(
                Arc::clone(&db),
                Arc::clone(&config),
                Arc::clone(&scan),
                Arc::clone(&health),
                Arc::clone(&waveform),
            );
            if !studio {
                check.start(app.handle());
            }
            if library_reset && !studio {
                Arc::clone(&scan).start(
                    app.handle().clone(),
                    Arc::clone(&db),
                    Arc::clone(&config),
                    Arc::clone(&waveform),
                );
            }
            let updater = Updater::new(app.handle().clone(), Arc::clone(&config));
            updater.start();
            // What another machine changed has copies here that nothing else
            // would refresh: the cue points the playlist holds, the list of
            // saved playlists, and — on the owner — the files that edits are
            // written to.
            let on_applied = {
                let handle = app.handle().clone();
                let db = Arc::clone(&db);
                let playlist = Arc::clone(&playlist);
                let tag_writer = Arc::clone(&tag_writer);
                Box::new(move |applied: &library::db::Applied| {
                    if applied.cue_points {
                        playlist.reload_cue_points();
                    }
                    if applied.playlists {
                        saved_playlists::emit(&handle, &db);
                    }
                    if !studio {
                        for id in &applied.edited_tracks {
                            tag_writer.request(*id);
                        }
                    }
                })
            };
            let hub = hub::Service::start(
                app.handle().clone(),
                role,
                Arc::clone(&db),
                &config,
                Arc::clone(&health),
                on_applied,
            );
            app.manage(AppState {
                db,
                config,
                updater,
                admin,
                session,
                scan,
                waveform,
                tag_backfill,
                health,
                check,
                tag_writer,
                bus,
                playlist,
                cue: Arc::new(Mutex::new(None)),
                cache,
                broadcast,
                app_handle: app.handle().clone(),
                data_dir: data_dir.clone(),
                library_reset,
                library_joined,
                hub,
            });
            Ok(())
        })
        .invoke_handler(admin_gated(tauri::generate_handler![
            search,
            get_track,
            get_tracks_by_ids,
            reveal_track,
            load_session,
            save_session,
            track_played,
            playlist_sync,
            playlist_add,
            playlist_add_front,
            playlist_insert,
            playlist_add_many,
            playlist_add_saved,
            playlist_save_as,
            saved_playlist_list,
            saved_playlist_get,
            saved_playlist_create,
            saved_playlist_export,
            saved_playlist_import,
            saved_playlist_add_entries,
            saved_playlist_remove_entry,
            saved_playlist_bind_entry,
            saved_playlist_move_entry,
            saved_playlist_remove_entries,
            saved_playlist_move_entries,
            saved_playlist_rename,
            saved_playlist_delete,
            playlist_set_item_cue_points,
            playlist_add_stop_marker,
            playlist_add_filler,
            playlist_remove,
            playlist_move,
            playlist_clear,
            playlist_play_index,
            playlist_play_now,
            playlist_next,
            playlist_prev,
            playlist_stop,
            playlist_set_auto_playlist,
            playlist_set_source,
            playlist_set_auto_advance,
            get_stats,
            get_all_paths,
            add_path,
            remove_path,
            locate_path,
            scan_libraries,
            cancel_scan,
            cancel_analysis,
            get_scan_status,
            purge_tracks,
            hide_tracks,
            unhide_tracks,
            recalculate_auto_cue,
            library_health,
            library_check_now,
            health_dismiss,
            health_undismiss,
            get_waveform_status,
            audio_list_devices,
            get_main_device,
            set_main_device,
            get_cue_device,
            set_cue_device,
            cue_load,
            cue_play,
            cue_pause,
            cue_stop,
            cue_seek,
            cue_set_volume,
            main_deck_is_playing,
            main_deck_play,
            main_deck_pause,
            main_deck_stop,
            main_deck_seek,
            main_deck_set_volume,
            main_deck_fade_out,
            main_deck_fade_to_next,
            get_waveform,
            get_waveform_detail,
            get_cover_art,
            get_shared_library,
            set_shared_library,
            read_ca_certificate,
            get_now_playing_config,
            set_now_playing_config,
            get_tuning_config,
            set_tuning_config,
            get_appearance,
            list_themes,
            set_theme,
            set_station_name,
            set_station_image,
            clear_station_image,
            set_spin_vinyl,
            reload_themes,
            reveal_themes_dir,
            now_playing_test,
            broadcast_shutdown,
            update_track_metadata,
            revert_track_tags,
            retry_tag_write,
            dismiss_tag_write,
            set_cue_points,
            admin_status,
            admin_unlock,
            admin_lock,
            admin_set_password,
            admin_clear_password,
            admin_set_idle_lock_min,
            update_status,
            update_check,
            update_install,
            open_link,
        ]))
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                shut_down(app);
            }
        });
}

/// What the app owes before its process ends. Runs on every exit, and from
/// the updater on Windows, where the installer ends the process without one.
pub(crate) fn shut_down(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        // Before the broadcast goes quiet: a queued transition may still owe
        // the airing log a play.
        state.playlist.drain();
        state.broadcast.shutdown_blocking();
    }
}
