use anyhow::{Context, Result};
use parking_lot::{Mutex, MutexGuard};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::atomic_write;
use crate::library::auto_cue::Thresholds;

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRef {
    pub name: String,
    pub description: String,
}

/// What this install is to a shared library. See `docs/shared-library.md`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LibraryRole {
    /// The library is this machine's own, and no hub is contacted.
    #[default]
    Standalone,
    /// This machine runs the scan and publishes the library to the hub.
    Owner,
}

/// A role this build does not know reads as standalone: the alternative is the
/// whole of `config.json` failing to parse over one word.
fn lenient_role<'de, D: serde::Deserializer<'de>>(d: D) -> Result<LibraryRole, D::Error> {
    let word = Option::<String>::deserialize(d)?;
    Ok(match word.as_deref() {
        Some("owner") => LibraryRole::Owner,
        _ => LibraryRole::Standalone,
    })
}

/// The shared-library section. `url` is a `postgresql://` connection URL with
/// its password, in plain text.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExternalLibraryConfig {
    #[serde(default, deserialize_with = "lenient_role")]
    pub role: LibraryRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// What the hub knows this install by. Written the first time it is needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<String>,
    /// What other machines call this one. Defaults to the start of its id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_name: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NowPlayingConfig {
    #[serde(default)]
    pub webhook_url: Option<String>,
    #[serde(default)]
    pub webhook_secret: Option<String>,
    #[serde(default)]
    pub file_dir: Option<String>,
    #[serde(default = "default_true")]
    pub file_enabled: bool,
    #[serde(default = "default_true")]
    pub webhook_enabled: bool,
}

fn default_true() -> bool {
    true
}

impl Default for NowPlayingConfig {
    fn default() -> Self {
        Self {
            webhook_url: None,
            webhook_secret: None,
            file_dir: None,
            file_enabled: true,
            webhook_enabled: true,
        }
    }
}

/// User-tunable playback behaviour. Each field carries `serde(default)` so a
/// `config.json` missing the section (or any single field) still loads. Values
/// are clamped to sane ranges by `normalize_tuning` whenever they are written.
#[derive(Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TuningConfig {
    #[serde(default)]
    pub interleave: InterleaveConfig,
    #[serde(default)]
    pub auto_playlist: AutoPlaylistConfig,
    #[serde(default)]
    pub rotation: RotationConfig,
    #[serde(default)]
    pub cache: CacheConfig,
    #[serde(default)]
    pub player: PlayerConfig,
    #[serde(default)]
    pub library: LibraryConfig,
    #[serde(default)]
    pub auto_cue: AutoCueConfig,
    #[serde(default)]
    pub updates: UpdatesConfig,
}

/// In-app update behaviour. See `docs/updates.md`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UpdatesConfig {
    /// Whether the app asks for a newer release by itself. Off, it only asks
    /// when an operator presses the button — for a station that must not
    /// reach the internet unprompted.
    #[serde(default = "default_apply")]
    pub auto_check: bool,
}

impl Default for UpdatesConfig {
    fn default() -> Self {
        Self { auto_check: true }
    }
}

/// Whether derived cue points are applied, and the levels the analyser works
/// to, in dBFS. Changing a level affects later analyses only: nothing already
/// stored is cleared, invalidated or re-decoded. See `docs/cue-auto-analysis.md`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AutoCueConfig {
    /// Whether a derived set takes effect. Switched off, analysis still runs
    /// and still stores its result — the station simply airs every track
    /// whole, so switching back on costs no second pass over the library. A
    /// manually prepared track is unaffected either way.
    #[serde(default = "default_apply")]
    pub apply: bool,
    /// Whether a derived Next Start takes effect. Switched off, a music track
    /// the analyser owns hands over at its Cue Out instead of overlapping the
    /// incoming item; the derived trims still apply and the derived position is
    /// still stored. Nested under `apply`, which hides the whole trio.
    #[serde(default = "default_apply")]
    pub apply_next_start: bool,
    /// Below this there is no programme audio, so Cue In and Cue Out trim it.
    #[serde(default = "default_silence_dbfs")]
    pub silence_dbfs: f64,
    /// Below this a music track is quiet enough for the next item to begin.
    #[serde(default = "default_segue_dbfs")]
    pub segue_dbfs: f64,
}

fn default_apply() -> bool {
    true
}
fn default_silence_dbfs() -> f64 {
    -70.0
}
fn default_segue_dbfs() -> f64 {
    -20.0
}

/// Range both thresholds are held to. The floor is below any mastering noise
/// floor; the ceiling keeps a mis-typed value from trimming audible programme.
pub const AUTO_CUE_DB_RANGE: std::ops::RangeInclusive<f64> = -100.0..=-3.0;

/// How far the segue threshold is pushed above the silence threshold when a
/// value would put it at or below one. The two answer different questions, and
/// a segue threshold under the silence threshold can never fire.
const AUTO_CUE_DB_GAP: f64 = 1.0;

impl Default for AutoCueConfig {
    fn default() -> Self {
        Self {
            apply: default_apply(),
            apply_next_start: default_apply(),
            silence_dbfs: default_silence_dbfs(),
            segue_dbfs: default_segue_dbfs(),
        }
    }
}

impl AutoCueConfig {
    pub fn thresholds(&self) -> Thresholds {
        Thresholds {
            silence_dbfs: self.silence_dbfs,
            segue_dbfs: self.segue_dbfs,
        }
    }
}

/// Playlist interleave cadence — how often jingles/commercials are woven into a
/// generated block of music, and how the commercial bucket is sized.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct InterleaveConfig {
    #[serde(default = "default_jingle_every")]
    pub jingle_every: i64,
    #[serde(default = "default_commercial_every")]
    pub commercial_every: i64,
    #[serde(default = "default_commercial_bucket_multiplier")]
    pub commercial_bucket_multiplier: i64,
    #[serde(default = "default_commercial_bucket_min")]
    pub commercial_bucket_min: i64,
}

fn default_jingle_every() -> i64 {
    4
}
fn default_commercial_every() -> i64 {
    8
}
fn default_commercial_bucket_multiplier() -> i64 {
    3
}
fn default_commercial_bucket_min() -> i64 {
    10
}

impl Default for InterleaveConfig {
    fn default() -> Self {
        Self {
            jingle_every: default_jingle_every(),
            commercial_every: default_commercial_every(),
            commercial_bucket_multiplier: default_commercial_bucket_multiplier(),
            commercial_bucket_min: default_commercial_bucket_min(),
        }
    }
}

/// Renderer-side auto-playlist + session tuning. Read by `state.svelte.ts`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AutoPlaylistConfig {
    #[serde(default = "default_auto_playlist_buffer")]
    pub auto_playlist_buffer: usize,
    #[serde(default = "default_auto_playlist_threshold")]
    pub auto_playlist_threshold: usize,
    /// How many aired tracks a snapshot carries for the History tab. Not a
    /// retention limit: every airing stays in `play_log`.
    #[serde(default = "default_history_cap")]
    pub history_cap: usize,
    #[serde(default = "default_session_save_throttle_ms")]
    pub session_save_throttle_ms: u64,
    #[serde(default = "default_net_retry_backoffs_ms")]
    pub net_retry_backoffs_ms: Vec<u64>,
}

fn default_auto_playlist_buffer() -> usize {
    20
}
fn default_auto_playlist_threshold() -> usize {
    5
}
fn default_history_cap() -> usize {
    100
}
fn default_session_save_throttle_ms() -> u64 {
    500
}
fn default_net_retry_backoffs_ms() -> Vec<u64> {
    vec![1000, 2000, 5000]
}

impl Default for AutoPlaylistConfig {
    fn default() -> Self {
        Self {
            auto_playlist_buffer: default_auto_playlist_buffer(),
            auto_playlist_threshold: default_auto_playlist_threshold(),
            history_cap: default_history_cap(),
            session_save_throttle_ms: default_session_save_throttle_ms(),
            net_retry_backoffs_ms: default_net_retry_backoffs_ms(),
        }
    }
}

/// Auto-playlist no-repeat windows, in minutes of wall clock against the airing
/// log. Music only. See `docs/rotation.md`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RotationConfig {
    /// A track that aired inside this window is not selected. 0 disables.
    #[serde(default = "default_title_window_min")]
    pub title_window_min: i64,
    /// A track whose artist aired inside this window is not selected. 0
    /// disables.
    #[serde(default = "default_artist_window_min")]
    pub artist_window_min: i64,
}

/// A week. A window longer than this cannot be honoured by any library the app
/// is likely to see, and would only turn every refill into a relaxation.
const ROTATION_WINDOW_MAX_MIN: i64 = 7 * 24 * 60;

fn default_title_window_min() -> i64 {
    180
}
fn default_artist_window_min() -> i64 {
    45
}

impl Default for RotationConfig {
    fn default() -> Self {
        Self {
            title_window_min: default_title_window_min(),
            artist_window_min: default_artist_window_min(),
        }
    }
}

/// Prefetch byte-cache tuning.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CacheConfig {
    #[serde(default = "default_max_cache_bytes")]
    pub max_cache_bytes: usize,
}

fn default_max_cache_bytes() -> usize {
    // Reuse the cache module's constant so the default cannot drift from it.
    crate::audio::cache::MAX_CACHE_BYTES
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_cache_bytes: default_max_cache_bytes(),
        }
    }
}

/// Audio-player network-resilience timeouts: read watchdog, output open-retry
/// cadence, and per-attempt read backoffs (all in milliseconds).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PlayerConfig {
    /// How long a track read may deliver **nothing** before the load is failed
    /// as a wedged mount. A budget with no progress, not a budget for the whole
    /// read: a large file on a slow share keeps arriving and keeps its load
    /// (#504).
    #[serde(default = "default_read_watchdog_timeout_ms")]
    pub read_watchdog_timeout_ms: u64,
    /// How long air may be silent waiting for a read before the load is given
    /// up on and the playlist skips to something resident. Far shorter than the
    /// watchdog, because it is measured against dead air rather than against a
    /// dying mount. It applies only to a load the playlist issued *and* will
    /// act on the failure of, on a deck that is on air with nothing audible:
    /// giving up is a recovery only if something else goes on instead, which
    /// rules out a track an operator put on air by hand and anything at all
    /// with auto-advance off (#504).
    #[serde(default = "default_dead_air_limit_ms")]
    pub dead_air_limit_ms: u64,
    #[serde(default = "default_open_retry_interval_ms")]
    pub open_retry_interval_ms: u64,
    #[serde(default = "default_read_retry_backoffs_ms")]
    pub read_retry_backoffs_ms: Vec<u64>,
    /// How long the live *Fade out* transport action takes to reach silence.
    #[serde(default = "default_fade_out_ms")]
    pub fade_out_ms: u64,
    /// How long the outgoing track takes to fade under the incoming one on
    /// *Fade to next*. Shorter than a fade to silence by default: a long ramp
    /// buries the track that just started.
    #[serde(default = "default_fade_to_next_ms")]
    pub fade_to_next_ms: u64,
    /// Whether a track is levelled to the ReplayGain reference on load.
    #[serde(default)]
    pub replay_gain: ReplayGainMode,
}

/// How much levelling the player applies to a loaded track.
///
/// There is deliberately no album mode. Album gain keeps a record's internal
/// level relationships, which is right for front-to-back listening and wrong
/// for radio: tracks air out of context from rotation, so it would reintroduce
/// exactly the variation this levels out. Airing a record in full is a
/// per-airing decision, not a station-wide setting.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub enum ReplayGainMode {
    /// Play every track at the level it was mastered.
    Off,
    /// Level each track to the reference on its own measurement.
    #[default]
    Track,
}

fn default_read_watchdog_timeout_ms() -> u64 {
    10_000
}
fn default_dead_air_limit_ms() -> u64 {
    3_000
}
fn default_open_retry_interval_ms() -> u64 {
    2_000
}
fn default_read_retry_backoffs_ms() -> Vec<u64> {
    vec![500, 1000, 2000]
}
fn default_fade_out_ms() -> u64 {
    4_000
}
fn default_fade_to_next_ms() -> u64 {
    2_500
}

/// Bounds on both fade durations. The floor keeps a "fade" from being an
/// indistinguishable cut; the ceiling keeps a mis-typed value from holding the
/// deck for a minute of inaudible ramp.
pub const FADE_MS_RANGE: std::ops::RangeInclusive<u64> = 200..=30_000;

impl Default for PlayerConfig {
    fn default() -> Self {
        Self {
            read_watchdog_timeout_ms: default_read_watchdog_timeout_ms(),
            dead_air_limit_ms: default_dead_air_limit_ms(),
            open_retry_interval_ms: default_open_retry_interval_ms(),
            read_retry_backoffs_ms: default_read_retry_backoffs_ms(),
            fade_out_ms: default_fade_out_ms(),
            fade_to_next_ms: default_fade_to_next_ms(),
            replay_gain: ReplayGainMode::default(),
        }
    }
}

/// Library health tuning.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LibraryConfig {
    /// Minutes between library checks; 0 turns the timer off.
    #[serde(default = "default_check_interval_min")]
    pub check_interval_min: u64,
    /// Whether a check that finds the disk changed may start a scan itself.
    /// Off by default: a check otherwise only ever reports.
    #[serde(default)]
    pub scan_on_changes: bool,
    /// Write metadata edits into the file's tags as well as the library.
    #[serde(default)]
    pub write_tags: bool,
    /// Seconds a tag write may take before it is reported as failed.
    #[serde(default = "default_tag_write_timeout_sec")]
    pub tag_write_timeout_sec: u64,
}

fn default_check_interval_min() -> u64 {
    15
}
fn default_tag_write_timeout_sec() -> u64 {
    30
}

impl Default for LibraryConfig {
    fn default() -> Self {
        Self {
            check_interval_min: default_check_interval_min(),
            scan_on_changes: false,
            write_tags: false,
            tag_write_timeout_sec: default_tag_write_timeout_sec(),
        }
    }
}

/// Admin mode. Delete `passwordHash` from `config.json` to recover a
/// forgotten password.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AdminConfig {
    /// Argon2 PHC string. `None` leaves admin mode always unlocked.
    #[serde(default)]
    pub password_hash: Option<String>,
    /// Minutes without input before admin mode locks again.
    #[serde(default = "default_idle_lock_min")]
    pub idle_lock_min: u64,
}

fn default_idle_lock_min() -> u64 {
    15
}

pub const IDLE_LOCK_MIN_RANGE: std::ops::RangeInclusive<u64> = 1..=240;

impl Default for AdminConfig {
    fn default() -> Self {
        Self {
            password_hash: None,
            idle_lock_min: default_idle_lock_min(),
        }
    }
}

/// Appearance: which theme is active, and the station's own name and images.
/// A theme never sets the station name — see `docs/theming.md`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AppearanceConfig {
    /// Directory name under `{app_data_dir}/themes`, or a built-in id.
    #[serde(default = "default_theme_id")]
    pub theme_id: String,
    #[serde(default)]
    pub station_name: Option<String>,
    /// File name inside `{app_data_dir}/branding`, never a path.
    #[serde(default)]
    pub logo: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    /// Whether the art at the centre of a deck's vinyl turns while it plays.
    #[serde(default = "default_true")]
    pub spin_vinyl: bool,
}

fn default_theme_id() -> String {
    "midnight".to_string()
}

/// Longest station name kept, in characters.
pub const STATION_NAME_MAX: usize = 64;

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            theme_id: default_theme_id(),
            station_name: None,
            logo: None,
            label: None,
            spin_vinyl: true,
        }
    }
}

#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    /// Where this machine reaches each library path, by the id the library
    /// knows it under. The paths themselves are library data
    /// (`library::roots`); only the folder is this machine's to say.
    #[serde(default)]
    pub library_mounts: BTreeMap<i64, String>,
    /// Library paths as they were kept before the library held them, and still
    /// the way to seed one by hand. Taken up at launch and then written out
    /// empty: see [`Config::adopt_mounts`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub music_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commercial_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jingle_paths: Vec<String>,
    #[serde(default)]
    pub main_device: Option<DeviceRef>,
    #[serde(default)]
    pub cue_device: Option<DeviceRef>,
    #[serde(default)]
    pub now_playing: NowPlayingConfig,
    #[serde(default)]
    pub tuning: TuningConfig,
    #[serde(default)]
    pub appearance: AppearanceConfig,
    #[serde(default)]
    pub admin: AdminConfig,
    #[serde(default)]
    pub external_library: ExternalLibraryConfig,
}

pub struct Config {
    path: PathBuf,
    inner: Mutex<AppConfig>,
    /// Stamped under `inner`, so sequence order is the order callers changed
    /// the config.
    next_save: Mutex<u64>,
    /// The sequence `config.json` holds, guarding the write. Taken only after
    /// `inner` is released, so a save in flight never makes another caller wait
    /// on the data lock.
    written: Mutex<u64>,
}

impl Config {
    pub fn open(dir: &Path) -> Result<Self> {
        let path = dir.join("config.json");
        let inner = Self::load(&path).unwrap_or_default();
        Ok(Self {
            path,
            inner: Mutex::new(inner),
            next_save: Mutex::new(0),
            written: Mutex::new(0),
        })
    }

    fn load(path: &Path) -> Option<AppConfig> {
        let raw = fs::read_to_string(path).ok()?;
        let mut parsed: serde_json::Value = serde_json::from_str(&raw).ok()?;
        if let Some(obj) = parsed.as_object_mut() {
            if !obj.contains_key("musicPaths") {
                if let Some(legacy) = obj.remove("libraryPaths") {
                    obj.insert("musicPaths".into(), legacy);
                }
            }
        }
        serde_json::from_value::<AppConfig>(parsed).ok()
    }

    /// Persist the config, releasing the data lock before touching the disk.
    ///
    /// The sequence is stamped while `cfg` is still held and the file lock is
    /// taken only after it is released: no caller ever waits on `inner` for a
    /// write, however many saves are in flight, which is what keeps
    /// `get_tuning()` answerable from a command that runs on the main thread.
    ///
    /// A save whose snapshot a later one overtook is skipped rather than
    /// written after it. Every snapshot is the whole config, so the newer one
    /// already carries this caller's change. See
    /// `docs/architecture.md#commands`.
    fn save_and_unlock(&self, cfg: MutexGuard<'_, AppConfig>) -> Result<()> {
        let seq = {
            let mut next = self.next_save.lock();
            *next += 1;
            *next
        };
        let snapshot = cfg.clone();
        drop(cfg);
        let mut written = self.written.lock();
        if seq <= *written {
            return Ok(());
        }
        let json = serde_json::to_string_pretty(&snapshot)?;
        atomic_write(&self.path, json.as_bytes()).context("write config.json")?;
        *written = seq;
        Ok(())
    }

    /// Library paths still held in the config, as `(content type, folder)` in
    /// listing order.
    pub fn legacy_paths(&self) -> Vec<(&'static str, String)> {
        let cfg = self.inner.lock();
        [
            ("music", &cfg.music_paths),
            ("commercial", &cfg.commercial_paths),
            ("jingle", &cfg.jingle_paths),
        ]
        .into_iter()
        .flat_map(|(kind, paths)| paths.iter().map(move |p| (kind, canonicalize_lossy(p))))
        .collect()
    }

    /// This machine's folder for each library path.
    pub fn mounts(&self) -> BTreeMap<i64, String> {
        self.inner.lock().library_mounts.clone()
    }

    /// Replace the folders and drop the paths [`Self::legacy_paths`] reported,
    /// in one write: a config that holds both would have them taken up twice.
    pub fn adopt_mounts(&self, mounts: BTreeMap<i64, String>) -> Result<()> {
        let mut cfg = self.inner.lock();
        cfg.library_mounts = mounts;
        cfg.music_paths.clear();
        cfg.commercial_paths.clear();
        cfg.jingle_paths.clear();
        self.save_and_unlock(cfg)
    }

    pub fn set_mount(&self, root_id: i64, dir: &str) -> Result<()> {
        let mut cfg = self.inner.lock();
        cfg.library_mounts.insert(root_id, dir.to_owned());
        self.save_and_unlock(cfg)
    }

    pub fn remove_mount(&self, root_id: i64) -> Result<()> {
        let mut cfg = self.inner.lock();
        cfg.library_mounts.remove(&root_id);
        self.save_and_unlock(cfg)
    }

    /// A folder as a library path stores it: canonical when it can be read,
    /// as given when it cannot.
    pub fn canonical_dir(dir: &str) -> String {
        canonicalize_lossy(dir)
    }

    pub fn get_main_device(&self) -> Option<DeviceRef> {
        self.inner.lock().main_device.clone()
    }

    pub fn set_main_device(&self, device: Option<DeviceRef>) -> Result<()> {
        let mut cfg = self.inner.lock();
        cfg.main_device = device;
        self.save_and_unlock(cfg)
    }

    pub fn get_cue_device(&self) -> Option<DeviceRef> {
        self.inner.lock().cue_device.clone()
    }

    pub fn set_cue_device(&self, device: Option<DeviceRef>) -> Result<()> {
        let mut cfg = self.inner.lock();
        cfg.cue_device = device;
        self.save_and_unlock(cfg)
    }

    pub fn external_library(&self) -> ExternalLibraryConfig {
        self.inner.lock().external_library.clone()
    }

    /// This install's id and name on the hub. The id is made and saved the
    /// first time it is asked for, and is this machine's from then on.
    pub fn machine(&self) -> Result<(String, String)> {
        let mut cfg = self.inner.lock();
        let section = &mut cfg.external_library;
        let fresh = section.machine_id.is_none();
        let id = section
            .machine_id
            .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone();
        let name = section
            .machine_name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| id.chars().take(8).collect());
        if fresh {
            self.save_and_unlock(cfg)?;
        }
        Ok((id, name))
    }

    pub fn get_now_playing(&self) -> NowPlayingConfig {
        self.inner.lock().now_playing.clone()
    }

    pub fn set_now_playing(&self, np: NowPlayingConfig) -> Result<()> {
        let mut cfg = self.inner.lock();
        cfg.now_playing = np;
        self.save_and_unlock(cfg)
    }

    /// The tuning section, normalized on the way out as well as in: a
    /// hand-edited `config.json` never reaches the code that acts on it with
    /// values outside their ranges.
    pub fn get_tuning(&self) -> TuningConfig {
        normalize_tuning(self.inner.lock().tuning.clone())
    }

    /// Persist a new tuning section. Values are clamped to sane ranges first, so
    /// a bad UI input (zero cadence, empty backoff list, tiny cache) can never
    /// wedge playlist generation or the player. Returns the clamped config the
    /// caller can echo back to the UI.
    pub fn set_tuning(&self, tuning: TuningConfig) -> Result<TuningConfig> {
        let mut cfg = self.inner.lock();
        cfg.tuning = normalize_tuning(tuning);
        let stored = cfg.tuning.clone();
        self.save_and_unlock(cfg)?;
        Ok(stored)
    }

    pub fn get_appearance(&self) -> AppearanceConfig {
        self.inner.lock().appearance.clone()
    }

    /// Persist a new appearance section. The station name is trimmed and capped,
    /// and an empty one is stored as `None`; the caller echoes the stored value
    /// back to the UI the way `set_tuning` does.
    pub fn set_appearance(&self, appearance: AppearanceConfig) -> Result<AppearanceConfig> {
        let mut cfg = self.inner.lock();
        cfg.appearance = normalize_appearance(appearance);
        let stored = cfg.appearance.clone();
        self.save_and_unlock(cfg)?;
        Ok(stored)
    }

    pub fn password_hash(&self) -> Option<String> {
        self.inner.lock().admin.password_hash.clone()
    }

    pub fn set_password_hash(&self, hash: Option<String>) -> Result<()> {
        let mut cfg = self.inner.lock();
        cfg.admin.password_hash = hash;
        self.save_and_unlock(cfg)
    }

    pub fn idle_lock_min(&self) -> u64 {
        let min = self.inner.lock().admin.idle_lock_min;
        min.clamp(*IDLE_LOCK_MIN_RANGE.start(), *IDLE_LOCK_MIN_RANGE.end())
    }

    /// Store the idle timeout, clamped to [`IDLE_LOCK_MIN_RANGE`]. Returns the
    /// stored value.
    pub fn set_idle_lock_min(&self, minutes: u64) -> Result<u64> {
        let minutes = minutes.clamp(*IDLE_LOCK_MIN_RANGE.start(), *IDLE_LOCK_MIN_RANGE.end());
        let mut cfg = self.inner.lock();
        cfg.admin.idle_lock_min = minutes;
        self.save_and_unlock(cfg)?;
        Ok(minutes)
    }
}

/// Trim and cap the station name; an empty name means "use the product name".
fn normalize_appearance(mut appearance: AppearanceConfig) -> AppearanceConfig {
    appearance.station_name = appearance
        .station_name
        .map(|name| {
            name.trim()
                .chars()
                .take(STATION_NAME_MAX)
                .collect::<String>()
        })
        .filter(|name| !name.is_empty());
    appearance
}

/// Clamp tuning values to ranges that keep the backend and renderer safe:
/// cadences/counters must be >= 1 (a `0` would divide-by-zero or spin), the
/// refill threshold cannot exceed its buffer, the cache needs a floor so at
/// least one track can stay resident, and retry/backoff lists must be non-empty
/// with non-zero delays. Defaults are already in range, so an untouched config
/// is unchanged.
fn normalize_tuning(mut t: TuningConfig) -> TuningConfig {
    let il = &mut t.interleave;
    // 0 disables jingle/commercial insertion entirely.
    il.jingle_every = il.jingle_every.max(0);
    il.commercial_every = il.commercial_every.max(0);
    il.commercial_bucket_multiplier = il.commercial_bucket_multiplier.max(1);
    il.commercial_bucket_min = il.commercial_bucket_min.max(0);

    let ap = &mut t.auto_playlist;
    ap.auto_playlist_buffer = ap.auto_playlist_buffer.max(1);
    ap.auto_playlist_threshold = ap.auto_playlist_threshold.clamp(1, ap.auto_playlist_buffer);
    ap.history_cap = ap.history_cap.max(1);
    if ap.net_retry_backoffs_ms.is_empty() {
        ap.net_retry_backoffs_ms = default_net_retry_backoffs_ms();
    } else {
        for b in &mut ap.net_retry_backoffs_ms {
            *b = (*b).max(1);
        }
    }

    let rot = &mut t.rotation;
    rot.title_window_min = rot.title_window_min.clamp(0, ROTATION_WINDOW_MAX_MIN);
    rot.artist_window_min = rot.artist_window_min.clamp(0, ROTATION_WINDOW_MAX_MIN);

    // Floor of 16 MiB: enough for at least one whole track to stay resident.
    t.cache.max_cache_bytes = t.cache.max_cache_bytes.max(16 * 1024 * 1024);

    let p = &mut t.player;
    p.read_watchdog_timeout_ms = p.read_watchdog_timeout_ms.max(1);
    p.dead_air_limit_ms = p.dead_air_limit_ms.max(1);
    p.open_retry_interval_ms = p.open_retry_interval_ms.max(1);
    if p.read_retry_backoffs_ms.is_empty() {
        p.read_retry_backoffs_ms = default_read_retry_backoffs_ms();
    } else {
        for b in &mut p.read_retry_backoffs_ms {
            *b = (*b).max(1);
        }
    }
    p.fade_out_ms = p
        .fade_out_ms
        .clamp(*FADE_MS_RANGE.start(), *FADE_MS_RANGE.end());
    p.fade_to_next_ms = p
        .fade_to_next_ms
        .clamp(*FADE_MS_RANGE.start(), *FADE_MS_RANGE.end());

    t.library.tag_write_timeout_sec = t.library.tag_write_timeout_sec.clamp(5, 300);

    let ac = &mut t.auto_cue;
    let (lo, hi) = (*AUTO_CUE_DB_RANGE.start(), *AUTO_CUE_DB_RANGE.end());
    ac.silence_dbfs = clamp_db(
        ac.silence_dbfs,
        default_silence_dbfs(),
        lo,
        hi - AUTO_CUE_DB_GAP,
    );
    ac.segue_dbfs = clamp_db(
        ac.segue_dbfs,
        default_segue_dbfs(),
        ac.silence_dbfs + AUTO_CUE_DB_GAP,
        hi,
    );

    t
}

/// Round a dBFS setting to whole decibels and clamp it, falling back to
/// `fallback` for a value that is not a number at all — `clamp` panics on NaN,
/// and `config.json` is a text file an operator may have edited by hand.
///
/// Whole decibels are what the analyser resolves: a decode is reduced to the
/// level of each window, at whole levels in
/// `level_envelope::LEVEL_MIN_DBFS..=LEVEL_MAX_DBFS`, so a later threshold change
/// re-derive a track's markers without reading the file again. A fractional
/// threshold would fall between two levels and the stored envelope would stop
/// answering it exactly. The number inputs have always offered whole steps, so
/// this only folds a hand-edited `-70.5`.
fn clamp_db(v: f64, fallback: f64, lo: f64, hi: f64) -> f64 {
    if v.is_nan() {
        fallback
    } else {
        v.round().clamp(lo, hi)
    }
}

fn canonicalize_lossy(p: &str) -> String {
    fs::canonicalize(p)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| p.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn a_library_is_standalone_until_told_otherwise() {
        let dir = tempdir().unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        assert_eq!(cfg.external_library(), ExternalLibraryConfig::default());
        assert_eq!(cfg.external_library().role, LibraryRole::Standalone);
    }

    #[test]
    fn a_role_this_build_does_not_know_keeps_the_rest_of_the_config() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("config.json"),
            r#"{"libraryMounts": {"3": "/mnt/radio"},
                "externalLibrary": {"role": "studio", "url": "postgresql://hub/x"}}"#,
        )
        .unwrap();

        let cfg = Config::open(dir.path()).unwrap();

        assert_eq!(cfg.mounts().len(), 1);
        let section = cfg.external_library();
        assert_eq!(section.role, LibraryRole::Standalone);
        assert_eq!(section.url.as_deref(), Some("postgresql://hub/x"));
    }

    #[test]
    fn the_machine_id_is_made_once_and_kept() {
        let dir = tempdir().unwrap();
        let (id, name) = Config::open(dir.path()).unwrap().machine().unwrap();
        assert_eq!(id.len(), 36);
        assert_eq!(name, id[..8]);

        let again = Config::open(dir.path()).unwrap();
        assert_eq!(again.machine().unwrap().0, id);
    }

    #[test]
    fn load_defaults_when_missing() {
        let dir = tempdir().unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        assert!(cfg.mounts().is_empty());
        assert!(cfg.legacy_paths().is_empty());
    }

    #[test]
    fn add_remove_round_trips_to_disk() {
        let dir = tempdir().unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        cfg.set_mount(3, "/mnt/radio").unwrap();

        // reopen reads persisted state
        drop(cfg);
        let cfg2 = Config::open(dir.path()).unwrap();
        assert_eq!(
            cfg2.mounts(),
            BTreeMap::from([(3, "/mnt/radio".to_owned())])
        );

        cfg2.remove_mount(3).unwrap();
        assert!(Config::open(dir.path()).unwrap().mounts().is_empty());
    }

    #[test]
    fn taking_up_the_legacy_paths_empties_them_on_disk() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("config.json"),
            r#"{"musicPaths": ["/m"], "jinglePaths": ["/j"]}"#,
        )
        .unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        assert_eq!(
            cfg.legacy_paths(),
            vec![("music", "/m".to_owned()), ("jingle", "/j".to_owned())]
        );

        cfg.adopt_mounts(BTreeMap::from([(1, "/m".to_owned()), (2, "/j".to_owned())]))
            .unwrap();

        let reopened = Config::open(dir.path()).unwrap();
        assert!(reopened.legacy_paths().is_empty());
        assert_eq!(reopened.mounts().len(), 2);
        let raw = fs::read_to_string(dir.path().join("config.json")).unwrap();
        assert!(!raw.contains("musicPaths"));
    }

    #[test]
    fn concurrent_saves_leave_a_whole_file() {
        let dir = tempdir().unwrap();
        let path = dir.path();
        let config = Config::open(path).unwrap();
        std::thread::scope(|scope| {
            for n in 1..=8u64 {
                let config = &config;
                scope.spawn(move || {
                    for _ in 0..25 {
                        let mut tuning = TuningConfig::default();
                        // Both values carry the writer's number, and
                        // `history_cap` defaults to 100, so a file a reader
                        // caught half-written — which `Config::load` turns into
                        // defaults — cannot pass for a whole one.
                        tuning.auto_playlist.history_cap = n as usize;
                        tuning.auto_playlist.session_save_throttle_ms = n;
                        config.set_tuning(tuning).unwrap();
                        let ap = Config::open(path).unwrap().get_tuning().auto_playlist;
                        assert!((1..=8).contains(&ap.history_cap));
                        assert_eq!(ap.history_cap as u64, ap.session_save_throttle_ms);
                    }
                });
            }
        });
        // What a restart reads back is the last state memory holds: the writes
        // reached the file in the order they changed the config.
        assert_eq!(
            Config::open(path).unwrap().get_tuning(),
            config.get_tuning()
        );
        assert!(!path.join("config.json.tmp").exists());
    }

    #[test]
    fn a_save_a_newer_one_overtook_is_skipped() {
        let dir = tempdir().unwrap();
        let config = Config::open(dir.path()).unwrap();
        // A later save already reached the file. Its snapshot is the whole
        // config, so it carries this change too — writing after it would put
        // the older state back.
        *config.written.lock() = 5;
        let mut cfg = config.inner.lock();
        cfg.tuning.auto_playlist.history_cap = 7;
        config.save_and_unlock(cfg).unwrap();
        assert!(!dir.path().join("config.json").exists());

        *config.next_save.lock() = 5;
        let mut cfg = config.inner.lock();
        cfg.tuning.auto_playlist.history_cap = 9;
        config.save_and_unlock(cfg).unwrap();
        let reloaded = Config::open(dir.path()).unwrap().get_tuning();
        assert_eq!(reloaded.auto_playlist.history_cap, 9);
    }

    #[test]
    fn a_read_answers_while_a_save_waits_for_the_file() {
        let dir = tempdir().unwrap();
        let config = Config::open(dir.path()).unwrap();
        // Hold the file lock: a save gets as far as stamping its sequence under
        // the data lock, then parks here with the data lock released.
        let file = config.written.lock();
        std::thread::scope(|scope| {
            let config = &config;
            scope.spawn(move || config.set_tuning(TuningConfig::default()).unwrap());
            // The stamp is taken under the data lock, so seeing it means the
            // save is on its way to the file.
            while *config.next_save.lock() == 0 {
                std::thread::yield_now();
            }
            let (tx, rx) = std::sync::mpsc::channel();
            scope.spawn(move || tx.send(config.get_tuning()).unwrap());
            // A read that waits here is the data lock being held across the
            // write, whatever else is in flight.
            rx.recv_timeout(std::time::Duration::from_secs(5))
                .expect("a read waited on a save");
            drop(file);
        });
    }

    #[test]
    fn migrates_legacy_library_paths_field() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"libraryPaths": ["/old"]}"#).unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        assert_eq!(cfg.legacy_paths(), vec![("music", "/old".to_string())]);
    }

    #[test]
    fn missing_device_fields_default_to_none() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"musicPaths": ["/a"]}"#).unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        assert!(cfg.get_main_device().is_none());
        assert!(cfg.get_cue_device().is_none());
    }

    #[test]
    fn now_playing_defaults_when_missing() {
        let dir = tempdir().unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        let np = cfg.get_now_playing();
        assert_eq!(np, NowPlayingConfig::default());
        assert!(np.file_enabled);
        assert!(np.webhook_enabled);
        assert!(np.webhook_url.is_none());
    }

    #[test]
    fn now_playing_round_trips_to_disk() {
        let dir = tempdir().unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        let np = NowPlayingConfig {
            webhook_url: Some("https://example.com/hook".into()),
            webhook_secret: Some("s3cret".into()),
            file_dir: Some("/tmp/np".into()),
            file_enabled: false,
            webhook_enabled: true,
        };
        cfg.set_now_playing(np.clone()).unwrap();

        drop(cfg);
        let cfg2 = Config::open(dir.path()).unwrap();
        assert_eq!(cfg2.get_now_playing(), np);
    }

    #[test]
    fn now_playing_partial_json_fills_defaults() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"nowPlaying":{"webhookUrl":"https://x"}}"#).unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        let np = cfg.get_now_playing();
        assert_eq!(np.webhook_url.as_deref(), Some("https://x"));
        assert!(np.file_enabled);
        assert!(np.webhook_enabled);
    }

    #[test]
    fn auto_cue_thresholds_default_to_the_documented_levels() {
        let dir = tempdir().unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        let ac = cfg.get_tuning().auto_cue;
        assert_eq!(ac.silence_dbfs, -70.0);
        assert_eq!(ac.segue_dbfs, -20.0);
    }

    /// Both switches are on out of the box, and a `config.json` written before
    /// the Next Start switch existed keeps the behaviour it had.
    #[test]
    fn both_auto_cue_switches_default_on() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("config.json"),
            r#"{"tuning":{"autoCue":{"apply":true}}}"#,
        )
        .unwrap();

        let ac = Config::open(dir.path()).unwrap().get_tuning().auto_cue;
        assert!(ac.apply);
        assert!(ac.apply_next_start);
        assert!(AutoCueConfig::default().apply_next_start);
    }

    #[test]
    fn the_next_start_switch_round_trips() {
        let dir = tempdir().unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        let stored = cfg
            .set_tuning(TuningConfig {
                auto_cue: AutoCueConfig {
                    apply_next_start: false,
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
        assert!(stored.auto_cue.apply);
        assert!(!stored.auto_cue.apply_next_start);

        let reopened = Config::open(dir.path()).unwrap().get_tuning();
        assert!(reopened.auto_cue.apply);
        assert!(!reopened.auto_cue.apply_next_start);
    }

    /// The two thresholds answer different questions, and a segue threshold at
    /// or below the silence threshold could never fire.
    #[test]
    fn a_segue_threshold_under_the_silence_threshold_is_lifted_above_it() {
        let dir = tempdir().unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        let stored = cfg
            .set_tuning(TuningConfig {
                auto_cue: AutoCueConfig {
                    apply: true,
                    apply_next_start: true,
                    silence_dbfs: -40.0,
                    segue_dbfs: -55.0,
                },
                ..Default::default()
            })
            .unwrap();
        assert_eq!(stored.auto_cue.silence_dbfs, -40.0);
        assert_eq!(stored.auto_cue.segue_dbfs, -39.0);
    }

    /// The analyser resolves whole levels only, so a threshold has to land on
    /// one. Only a hand-edited `config.json` can carry a fraction —
    /// the inputs step in whole decibels.
    #[test]
    fn a_fractional_threshold_is_rounded_to_a_whole_decibel() {
        let dir = tempdir().unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        let stored = cfg
            .set_tuning(TuningConfig {
                auto_cue: AutoCueConfig {
                    silence_dbfs: -70.5,
                    segue_dbfs: -20.4,
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
        assert_eq!(stored.auto_cue.silence_dbfs, -71.0, "-70.5 rounds away");
        assert_eq!(stored.auto_cue.segue_dbfs, -20.0);
        assert!(stored.auto_cue.segue_dbfs >= stored.auto_cue.silence_dbfs + AUTO_CUE_DB_GAP);
    }

    #[test]
    fn auto_cue_thresholds_are_held_to_their_range() {
        let dir = tempdir().unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        let stored = cfg
            .set_tuning(TuningConfig {
                auto_cue: AutoCueConfig {
                    apply: true,
                    apply_next_start: true,
                    silence_dbfs: -400.0,
                    segue_dbfs: 12.0,
                },
                ..Default::default()
            })
            .unwrap();
        assert_eq!(stored.auto_cue.silence_dbfs, *AUTO_CUE_DB_RANGE.start());
        assert_eq!(stored.auto_cue.segue_dbfs, *AUTO_CUE_DB_RANGE.end());
    }

    /// `config.json` is a text file an operator may edit by hand, and nothing
    /// normalizes it on the way in — so the read has to. A segue threshold
    /// under the silence one would otherwise make every music track a cold
    /// ending, permanently: the results are stamped `auto`, and a threshold
    /// change never re-analyses.
    #[test]
    fn a_hand_edited_threshold_is_normalized_on_the_way_out() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("config.json"),
            r#"{"tuning":{"autoCue":{"silenceDbfs":-20.0,"segueDbfs":-70.0}}}"#,
        )
        .unwrap();

        let ac = Config::open(dir.path()).unwrap().get_tuning().auto_cue;
        assert!(
            ac.segue_dbfs >= ac.silence_dbfs + AUTO_CUE_DB_GAP,
            "{ac:?} keeps the segue threshold above the silence one"
        );
    }

    /// `config.json` is a text file an operator may edit by hand, and `clamp`
    /// panics on NaN.
    #[test]
    fn a_non_numeric_threshold_falls_back_to_the_default() {
        let dir = tempdir().unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        let stored = cfg
            .set_tuning(TuningConfig {
                auto_cue: AutoCueConfig {
                    apply: true,
                    apply_next_start: true,
                    silence_dbfs: f64::NAN,
                    segue_dbfs: f64::NAN,
                },
                ..Default::default()
            })
            .unwrap();
        assert_eq!(stored.auto_cue.silence_dbfs, -70.0);
        assert_eq!(stored.auto_cue.segue_dbfs, -20.0);
    }

    #[test]
    fn appearance_defaults_when_missing() {
        let dir = tempdir().unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        assert_eq!(cfg.get_appearance(), AppearanceConfig::default());
        assert_eq!(cfg.get_appearance().theme_id, "midnight");
    }

    #[test]
    fn appearance_partial_json_fills_defaults() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("config.json"),
            r#"{"appearance":{"themeId":"station-red"}}"#,
        )
        .unwrap();

        let cfg = Config::open(dir.path()).unwrap();
        let appearance = cfg.get_appearance();
        assert_eq!(appearance.theme_id, "station-red");
        assert_eq!(appearance.station_name, None);
        assert_eq!(appearance.logo, None);
        assert!(appearance.spin_vinyl, "a config that predates it spins");
    }

    #[test]
    fn set_appearance_round_trips_to_disk() {
        let dir = tempdir().unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        cfg.set_appearance(AppearanceConfig {
            theme_id: "station-red".to_string(),
            station_name: Some("Radio Foo".to_string()),
            logo: Some("logo.svg".to_string()),
            label: None,
            spin_vinyl: false,
        })
        .unwrap();
        drop(cfg);

        let reopened = Config::open(dir.path()).unwrap();
        let appearance = reopened.get_appearance();
        assert_eq!(appearance.theme_id, "station-red");
        assert_eq!(appearance.station_name.as_deref(), Some("Radio Foo"));
        assert_eq!(appearance.logo.as_deref(), Some("logo.svg"));
        assert!(!appearance.spin_vinyl);
    }

    /// The station name is trimmed and capped, and an empty one means "use the
    /// product name" — the caller echoes the stored value back to the UI.
    #[test]
    fn set_appearance_normalizes_the_station_name() {
        let dir = tempdir().unwrap();
        let cfg = Config::open(dir.path()).unwrap();

        let stored = cfg
            .set_appearance(AppearanceConfig {
                station_name: Some("   ".to_string()),
                ..AppearanceConfig::default()
            })
            .unwrap();
        assert_eq!(stored.station_name, None, "a blank name is not a name");

        let stored = cfg
            .set_appearance(AppearanceConfig {
                station_name: Some(format!("  {}  ", "x".repeat(100))),
                ..AppearanceConfig::default()
            })
            .unwrap();
        assert_eq!(
            stored.station_name.unwrap().chars().count(),
            STATION_NAME_MAX
        );
    }

    #[test]
    fn tuning_defaults_when_missing() {
        let dir = tempdir().unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        assert_eq!(cfg.get_tuning(), TuningConfig::default());
    }

    #[test]
    fn tuning_partial_json_fills_defaults() {
        // Old config with only an interleave override still loads; every other
        // field falls back to its default (additive schema, no version bump).
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"tuning":{"interleave":{"jingleEvery":6}}}"#).unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        let t = cfg.get_tuning();
        assert_eq!(t.interleave.jingle_every, 6);
        assert_eq!(t.interleave.commercial_every, 8); // default
        assert_eq!(t.cache.max_cache_bytes, 150 * 1024 * 1024); // default
    }

    #[test]
    fn set_tuning_clamps_and_round_trips() {
        let dir = tempdir().unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        let mut t = TuningConfig::default();
        t.interleave.jingle_every = -3; // -> 0 (disabled floor)
        t.interleave.commercial_every = 0; // -> 0 (disabled, stays)
        t.auto_playlist.auto_playlist_buffer = 4;
        t.auto_playlist.auto_playlist_threshold = 99; // -> clamped to buffer (4)
        t.cache.max_cache_bytes = 1; // -> 16 MiB floor
        t.player.read_retry_backoffs_ms = vec![0, 5]; // 0 -> 1
        t.rotation.title_window_min = 99_999; // -> 7 days
        t.rotation.artist_window_min = -5; // -> 0 (rule off)
        let clamped = cfg.set_tuning(t).unwrap();
        assert_eq!(clamped.interleave.jingle_every, 0);
        assert_eq!(clamped.interleave.commercial_every, 0);
        assert_eq!(clamped.auto_playlist.auto_playlist_threshold, 4);
        assert_eq!(clamped.cache.max_cache_bytes, 16 * 1024 * 1024);
        assert_eq!(clamped.player.read_retry_backoffs_ms, vec![1, 5]);
        assert_eq!(clamped.rotation.title_window_min, 7 * 24 * 60);
        assert_eq!(clamped.rotation.artist_window_min, 0);

        // Reopen reads the persisted (clamped) values.
        drop(cfg);
        let cfg2 = Config::open(dir.path()).unwrap();
        assert_eq!(cfg2.get_tuning(), clamped);
    }

    #[test]
    fn admin_defaults_when_missing() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("config.json"), r#"{"musicPaths":[]}"#).unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        assert_eq!(cfg.password_hash(), None);
        assert_eq!(cfg.idle_lock_min(), 15);
    }

    #[test]
    fn admin_partial_json_fills_defaults() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("config.json"),
            r#"{"admin":{"passwordHash":"$argon2id$x"}}"#,
        )
        .unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        assert_eq!(cfg.password_hash().as_deref(), Some("$argon2id$x"));
        assert_eq!(cfg.idle_lock_min(), 15);
    }

    #[test]
    fn idle_lock_min_clamps_and_round_trips() {
        let dir = tempdir().unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        assert_eq!(cfg.set_idle_lock_min(0).unwrap(), 1);
        assert_eq!(cfg.set_idle_lock_min(10_000).unwrap(), 240);
        assert_eq!(cfg.set_idle_lock_min(30).unwrap(), 30);
        drop(cfg);
        assert_eq!(Config::open(dir.path()).unwrap().idle_lock_min(), 30);
    }

    #[test]
    fn device_set_get_round_trips_to_disk() {
        let dir = tempdir().unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        let device = DeviceRef {
            name: "hw:USB,0".to_string(),
            description: "Headphones".to_string(),
        };
        cfg.set_main_device(Some(device.clone())).unwrap();
        cfg.set_cue_device(Some(device.clone())).unwrap();

        drop(cfg);
        let cfg2 = Config::open(dir.path()).unwrap();
        assert_eq!(cfg2.get_main_device(), Some(device.clone()));
        assert_eq!(cfg2.get_cue_device(), Some(device));

        cfg2.set_main_device(None).unwrap();
        assert!(cfg2.get_main_device().is_none());
    }
}
