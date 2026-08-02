use parking_lot::Mutex;
use serde::Serialize;
use std::collections::HashSet;
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

mod audio;
mod broadcast;
mod library;
mod persist;
mod playlist;

use audio::cache::Cache;
use audio::devices::{list_output_devices, DeviceInfo};

pub const APP_NAME: &str = "RadiodioDJ";

use audio::player::{Cmd, PlayerHandle};
use broadcast::{service::default_now_playing_dir, BroadcastService};
use library::db::{Db, LibraryStats, Track, TrackMetadataUpdate};
use library::scan_state::{ScanState, ScanStatus, StartResult};
use library::waveform_scan::{WaveformJob, WaveformStatus};
use persist::config::{Config, DeviceRef, NowPlayingConfig};
use persist::session::{PlaylistItem, Session, SessionState};

pub struct AppState {
    db: Arc<Db>,
    config: Arc<Config>,
    session: Arc<Session>,
    scan: Arc<ScanState>,
    /// Background job that fills track waveforms after a metadata scan.
    waveform: Arc<WaveformJob>,
    main_deck: Arc<PlayerHandle>,
    cue: Arc<Mutex<Option<PlayerHandle>>>,
    /// Shared prefetch byte cache, resident in both deck players.
    cache: Arc<Cache>,
    broadcast: Arc<BroadcastService>,
    app_handle: AppHandle,
}

#[derive(Serialize)]
struct SessionLoadResult {
    state: SessionState,
    tracks: Vec<Track>,
}

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

#[tauri::command(rename_all = "camelCase")]
fn search(
    state: State<'_, AppState>,
    query: String,
    content_type: Option<String>,
    sort_by: Option<String>,
    sort_dir: Option<String>,
) -> Result<Vec<Track>, String> {
    state
        .db
        .search(
            &query,
            content_type.as_deref(),
            sort_by.as_deref(),
            sort_dir.as_deref(),
        )
        .map_err(err)
}

#[tauri::command(rename_all = "camelCase")]
fn get_track(state: State<'_, AppState>, id: i64) -> Result<Option<Track>, String> {
    state.db.get_track(id).map_err(err)
}

#[tauri::command(rename_all = "camelCase")]
fn get_tracks_by_ids(state: State<'_, AppState>, ids: Vec<i64>) -> Result<Vec<Track>, String> {
    state.db.get_tracks_by_ids(&ids).map_err(err)
}

#[tauri::command(rename_all = "camelCase")]
fn get_stats(state: State<'_, AppState>) -> Result<LibraryStats, String> {
    state.db.get_stats().map_err(err)
}

#[tauri::command(rename_all = "camelCase")]
fn track_played(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    state.db.increment_play_count(id).map_err(err)
}

#[tauri::command(rename_all = "camelCase")]
fn get_paths(state: State<'_, AppState>, r#type: String) -> Vec<String> {
    state.config.get_paths(&r#type)
}

#[tauri::command(rename_all = "camelCase")]
fn get_all_paths(state: State<'_, AppState>) -> serde_json::Value {
    state.config.get_all_paths()
}

#[tauri::command(rename_all = "camelCase")]
fn add_path(state: State<'_, AppState>, r#type: String, dir_path: String) -> Result<bool, String> {
    state.config.add_path(&r#type, &dir_path).map_err(err)
}

#[tauri::command(rename_all = "camelCase")]
fn remove_path(
    state: State<'_, AppState>,
    r#type: String,
    dir_path: String,
) -> Result<bool, String> {
    state.config.remove_path(&r#type, &dir_path).map_err(err)
}

#[tauri::command(rename_all = "camelCase")]
fn load_session(app: State<'_, AppState>) -> Result<SessionLoadResult, String> {
    let s = app.session.load();
    let mut ids: Vec<i64> = Vec::new();
    let mut seen: HashSet<i64> = HashSet::new();
    let item_ids = s.playlist_items.iter().filter_map(|item| match item {
        PlaylistItem::Track { id } => Some(id),
        PlaylistItem::Stop => None,
    });
    for id in s
        .playlist_ids
        .iter()
        .chain(item_ids)
        .chain(s.history_ids.iter())
        .chain(s.current_track_id.iter())
    {
        if seen.insert(*id) {
            ids.push(*id);
        }
    }
    let tracks = app.db.get_tracks_by_ids(&ids).map_err(err)?;
    Ok(SessionLoadResult { state: s, tracks })
}

#[tauri::command(rename_all = "camelCase")]
fn save_session(app: State<'_, AppState>, state: SessionState) -> Result<(), String> {
    app.session.save(state).map_err(err)
}

#[tauri::command(rename_all = "camelCase")]
fn generate_playlist(
    app: State<'_, AppState>,
    count: i64,
    exclude_ids: Vec<i64>,
) -> Result<Vec<Track>, String> {
    playlist::generate(&app.db, count, &exclude_ids).map_err(err)
}

#[tauri::command(rename_all = "camelCase")]
fn pick_filler(
    app: State<'_, AppState>,
    content_type: playlist::ContentType,
) -> Result<Option<Track>, String> {
    playlist::pick_filler(&app.db, content_type).map_err(err)
}

#[tauri::command(rename_all = "camelCase")]
fn main_deck_load(app_state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let track = app_state
        .db
        .get_track_broadcast_info(id)
        .map_err(err)?
        .ok_or_else(|| "track not found".to_string())?;
    let path = track.path.clone();
    let duration = track.duration;
    app_state.broadcast.set_pending_track(track.into());
    app_state.main_deck.send(Cmd::Load {
        id,
        path: std::path::PathBuf::from(path),
        duration: if duration > 0.0 { Some(duration) } else { None },
    });
    Ok(())
}

/// Update the prefetch residency window from the renderer's upcoming-track list.
/// Resolves each id's path from the DB and hands the window to the shared cache,
/// which evicts out-of-window entries and prefetches missing ones.
#[tauri::command(rename_all = "camelCase")]
fn main_deck_prefetch(app_state: State<'_, AppState>, ids: Vec<i64>) -> Result<(), String> {
    let window: Vec<(i64, std::path::PathBuf)> = app_state
        .db
        .get_paths_by_ids(&ids)
        .map_err(err)?
        .into_iter()
        .map(|(id, path)| (id, std::path::PathBuf::from(path)))
        .collect();
    app_state.cache.set_window(window);
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
fn main_deck_play(app_state: State<'_, AppState>) {
    app_state.main_deck.send(Cmd::Play);
}

#[tauri::command(rename_all = "camelCase")]
fn main_deck_pause(app_state: State<'_, AppState>) {
    app_state.main_deck.send(Cmd::Pause);
}

#[tauri::command(rename_all = "camelCase")]
fn main_deck_stop(app_state: State<'_, AppState>) {
    app_state.main_deck.send(Cmd::Stop);
}

#[tauri::command(rename_all = "camelCase")]
fn main_deck_seek(app_state: State<'_, AppState>, seconds: f64) {
    app_state.main_deck.send(Cmd::Seek(seconds));
}

#[tauri::command(rename_all = "camelCase")]
fn main_deck_set_volume(app_state: State<'_, AppState>, volume: f32) {
    app_state.main_deck.send(Cmd::SetVolume(volume));
}

/// Return a track's stored amplitude-curve peaks (one byte per bucket) for the
/// seek UI, or `None` when the track has no waveform. Deck-agnostic — both the
/// main and cue decks render the same per-track curve.
#[tauri::command(rename_all = "camelCase")]
fn get_waveform(app_state: State<'_, AppState>, id: i64) -> Result<Option<Vec<u8>>, String> {
    app_state.db.get_waveform(id).map_err(err)
}

/// Extract a track's embedded cover art as a base64 `data:` URL for the deck's
/// vinyl disc, or `None` when the file has no artwork. Read on demand (like the
/// waveform) rather than stored, so the library DB stays free of image blobs.
#[tauri::command(rename_all = "camelCase")]
fn get_cover_art(app_state: State<'_, AppState>, id: i64) -> Result<Option<String>, String> {
    let media = app_state.db.get_media_track(id).map_err(err)?;
    Ok(media.and_then(|m| library::scanner::read_cover_art(&m.path)))
}

#[tauri::command(rename_all = "camelCase")]
fn audio_list_devices() -> Vec<DeviceInfo> {
    list_output_devices()
}

#[tauri::command(rename_all = "camelCase")]
fn update_track_metadata(
    state: State<'_, AppState>,
    updates: TrackMetadataUpdate,
) -> Result<Track, String> {
    state.db.update_track_metadata(&updates).map_err(err)
}

#[tauri::command(rename_all = "camelCase")]
fn get_main_device(state: State<'_, AppState>) -> Option<DeviceRef> {
    state.config.get_main_device()
}

#[tauri::command(rename_all = "camelCase")]
fn set_main_device(state: State<'_, AppState>, device: Option<DeviceRef>) -> Result<(), String> {
    state.config.set_main_device(device).map_err(err)?;
    log::info!("main device updated; restart required to apply");
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
fn get_cue_device(state: State<'_, AppState>) -> Option<DeviceRef> {
    state.config.get_cue_device()
}

#[tauri::command(rename_all = "camelCase")]
fn set_cue_device(state: State<'_, AppState>, device: Option<DeviceRef>) -> Result<(), String> {
    state.config.set_cue_device(device).map_err(err)?;
    // Invalidate cached cue handle so the next cue_* command spawns
    // against the new device. Dropping the Sender stops the worker thread.
    *state.cue.lock() = None;
    Ok(())
}

/// Ensure a cue `PlayerHandle` exists for the configured cue device.
/// Lazy-spawned on first cue command. Errors if no cue device is set
/// or the saved device cannot be resolved.
fn with_cue<F>(state: &State<'_, AppState>, f: F) -> Result<(), String>
where
    F: FnOnce(&PlayerHandle),
{
    let mut guard = state.cue.lock();
    if guard.is_none() {
        let cue_ref = state
            .config
            .get_cue_device()
            .ok_or_else(|| "no cue device configured; pick one in Settings → Audio".to_string())?;
        // The worker resolves the DeviceRef lazily and falls back to the default
        // output, mirroring the main deck's self-healing open (#259).
        *guard = Some(PlayerHandle::spawn(
            state.app_handle.clone(),
            Some(cue_ref),
            "cue",
            Arc::clone(&state.cache),
        ));
    }
    if let Some(handle) = guard.as_ref() {
        f(handle);
    }
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
fn cue_load(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let track = state
        .db
        .get_media_track(id)
        .map_err(err)?
        .ok_or_else(|| "track not found".to_string())?;
    with_cue(&state, |h| {
        h.send(Cmd::Load {
            id,
            path: std::path::PathBuf::from(track.path),
            duration: if track.duration > 0.0 {
                Some(track.duration)
            } else {
                None
            },
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

#[tauri::command(rename_all = "camelCase")]
fn get_now_playing_config(state: State<'_, AppState>) -> NowPlayingConfig {
    state.config.get_now_playing()
}

#[tauri::command(rename_all = "camelCase")]
fn set_now_playing_config(
    state: State<'_, AppState>,
    config: NowPlayingConfig,
) -> Result<(), String> {
    state.config.set_now_playing(config).map_err(err)
}

#[tauri::command(rename_all = "camelCase")]
fn now_playing_test(state: State<'_, AppState>) -> Result<u16, String> {
    state.broadcast.test_webhook_blocking()
}

#[tauri::command(rename_all = "camelCase")]
fn broadcast_shutdown(state: State<'_, AppState>) {
    state.broadcast.shutdown_blocking();
}

#[tauri::command(rename_all = "camelCase")]
fn scan_libraries(app: AppHandle, state: State<'_, AppState>) -> StartResult {
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
}

#[tauri::command(rename_all = "camelCase")]
fn get_scan_status(state: State<'_, AppState>) -> ScanStatus {
    state.scan.status()
}

#[tauri::command(rename_all = "camelCase")]
fn get_waveform_status(state: State<'_, AppState>) -> WaveformStatus {
    state.waveform.status()
}

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
                .level_for("symphonia_bundle_mp3", log::LevelFilter::Warn)
                .max_file_size(1024 * 1024)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepOne)
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
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
            let db = Db::open(&data_dir.join("radiodiodj.db"))?;
            let config = Config::open(&data_dir)?;
            let session = Session::open(&data_dir);
            // Pass the saved DeviceRef (not a pre-resolved device): the worker
            // resolves it lazily on the audio thread and falls back to the
            // system default, so a device unavailable at launch no longer kills
            // playback for the whole session (#259).
            let main_device = config.get_main_device();
            let cache = Cache::new(app.handle().clone());
            let main_deck = PlayerHandle::spawn(
                app.handle().clone(),
                main_device,
                "main-deck",
                Arc::clone(&cache),
            );
            let config = Arc::new(config);
            let broadcast = Arc::new(BroadcastService::new(
                Arc::clone(&config),
                default_now_playing_dir(&data_dir),
            )?);
            broadcast.attach_to_app(app.handle());
            let db = Arc::new(db);
            let waveform = Arc::new(WaveformJob::default());
            // Backfill waveforms for any already-indexed tracks that lack one,
            // without waiting for the next scan. No-op on an empty library.
            Arc::clone(&waveform).start(app.handle().clone(), Arc::clone(&db));
            app.manage(AppState {
                db,
                config,
                session: Arc::new(session),
                scan: Arc::new(ScanState::default()),
                waveform,
                main_deck: Arc::new(main_deck),
                cue: Arc::new(Mutex::new(None)),
                cache,
                broadcast,
                app_handle: app.handle().clone(),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            search,
            get_track,
            get_tracks_by_ids,
            load_session,
            save_session,
            track_played,
            generate_playlist,
            pick_filler,
            get_stats,
            get_paths,
            get_all_paths,
            add_path,
            remove_path,
            scan_libraries,
            cancel_scan,
            get_scan_status,
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
            main_deck_load,
            main_deck_prefetch,
            main_deck_play,
            main_deck_pause,
            main_deck_stop,
            main_deck_seek,
            main_deck_set_volume,
            get_waveform,
            get_cover_art,
            get_now_playing_config,
            set_now_playing_config,
            now_playing_test,
            broadcast_shutdown,
            update_track_metadata,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                if let Some(state) = app.try_state::<AppState>() {
                    state.broadcast.shutdown_blocking();
                }
            }
        });
}
