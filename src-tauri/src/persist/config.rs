use anyhow::{Context, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRef {
    pub name: String,
    pub description: String,
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
#[derive(Serialize, Deserialize, Default, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TuningConfig {
    #[serde(default)]
    pub interleave: InterleaveConfig,
    #[serde(default)]
    pub auto_playlist: AutoPlaylistConfig,
    #[serde(default)]
    pub cache: CacheConfig,
    #[serde(default)]
    pub player: PlayerConfig,
    #[serde(default)]
    pub library: LibraryConfig,
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
    #[serde(default = "default_read_watchdog_timeout_ms")]
    pub read_watchdog_timeout_ms: u64,
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
        }
    }
}

#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    #[serde(default)]
    pub music_paths: Vec<String>,
    #[serde(default)]
    pub commercial_paths: Vec<String>,
    #[serde(default)]
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
}

pub struct Config {
    path: PathBuf,
    inner: Mutex<AppConfig>,
}

impl Config {
    pub fn open(dir: &Path) -> Result<Self> {
        let path = dir.join("config.json");
        let inner = Self::load(&path).unwrap_or_default();
        Ok(Self {
            path,
            inner: Mutex::new(inner),
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

    fn save_locked(&self, cfg: &AppConfig) -> Result<()> {
        let json = serde_json::to_string_pretty(cfg)?;
        fs::write(&self.path, json).context("write config.json")?;
        Ok(())
    }

    pub fn get_paths(&self, kind: &str) -> Vec<String> {
        let cfg = self.inner.lock();
        match kind {
            "music" => cfg.music_paths.clone(),
            "commercial" => cfg.commercial_paths.clone(),
            "jingle" => cfg.jingle_paths.clone(),
            _ => vec![],
        }
    }

    pub fn get_all_paths(&self) -> serde_json::Value {
        let cfg = self.inner.lock();
        serde_json::json!({
            "music": cfg.music_paths,
            "commercial": cfg.commercial_paths,
            "jingle": cfg.jingle_paths,
        })
    }

    pub fn add_path(&self, kind: &str, dir_path: &str) -> Result<bool> {
        let resolved = canonicalize_lossy(dir_path);
        let mut cfg = self.inner.lock();
        let arr = match kind {
            "music" => &mut cfg.music_paths,
            "commercial" => &mut cfg.commercial_paths,
            "jingle" => &mut cfg.jingle_paths,
            _ => return Ok(false),
        };
        if arr.contains(&resolved) {
            return Ok(false);
        }
        arr.push(resolved);
        self.save_locked(&cfg)?;
        Ok(true)
    }

    pub fn get_main_device(&self) -> Option<DeviceRef> {
        self.inner.lock().main_device.clone()
    }

    pub fn set_main_device(&self, device: Option<DeviceRef>) -> Result<()> {
        let mut cfg = self.inner.lock();
        cfg.main_device = device;
        self.save_locked(&cfg)
    }

    pub fn get_cue_device(&self) -> Option<DeviceRef> {
        self.inner.lock().cue_device.clone()
    }

    pub fn set_cue_device(&self, device: Option<DeviceRef>) -> Result<()> {
        let mut cfg = self.inner.lock();
        cfg.cue_device = device;
        self.save_locked(&cfg)
    }

    pub fn get_now_playing(&self) -> NowPlayingConfig {
        self.inner.lock().now_playing.clone()
    }

    pub fn set_now_playing(&self, np: NowPlayingConfig) -> Result<()> {
        let mut cfg = self.inner.lock();
        cfg.now_playing = np;
        self.save_locked(&cfg)
    }

    pub fn get_tuning(&self) -> TuningConfig {
        self.inner.lock().tuning.clone()
    }

    /// Persist a new tuning section. Values are clamped to sane ranges first, so
    /// a bad UI input (zero cadence, empty backoff list, tiny cache) can never
    /// wedge playlist generation or the player. Returns the clamped config the
    /// caller can echo back to the UI.
    pub fn set_tuning(&self, tuning: TuningConfig) -> Result<TuningConfig> {
        let mut cfg = self.inner.lock();
        cfg.tuning = normalize_tuning(tuning);
        self.save_locked(&cfg)?;
        Ok(cfg.tuning.clone())
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
        self.save_locked(&cfg)?;
        Ok(cfg.appearance.clone())
    }

    pub fn password_hash(&self) -> Option<String> {
        self.inner.lock().admin.password_hash.clone()
    }

    pub fn set_password_hash(&self, hash: Option<String>) -> Result<()> {
        let mut cfg = self.inner.lock();
        cfg.admin.password_hash = hash;
        self.save_locked(&cfg)
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
        self.save_locked(&cfg)?;
        Ok(minutes)
    }

    pub fn remove_path(&self, kind: &str, dir_path: &str) -> Result<bool> {
        let resolved = canonicalize_lossy(dir_path);
        let mut cfg = self.inner.lock();
        let arr = match kind {
            "music" => &mut cfg.music_paths,
            "commercial" => &mut cfg.commercial_paths,
            "jingle" => &mut cfg.jingle_paths,
            _ => return Ok(false),
        };
        if let Some(idx) = arr.iter().position(|p| p == &resolved) {
            arr.remove(idx);
            self.save_locked(&cfg)?;
            return Ok(true);
        }
        Ok(false)
    }
}

/// Clamp tuning values to ranges that keep the backend and renderer safe:
/// cadences/counters must be >= 1 (a `0` would divide-by-zero or spin), the
/// refill threshold cannot exceed its buffer, the cache needs a floor so at
/// least one track can stay resident, and retry/backoff lists must be non-empty
/// with non-zero delays. Defaults are already in range, so an untouched config
/// is unchanged.
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

    // Floor of 16 MiB: enough for at least one whole track to stay resident.
    t.cache.max_cache_bytes = t.cache.max_cache_bytes.max(16 * 1024 * 1024);

    let p = &mut t.player;
    p.read_watchdog_timeout_ms = p.read_watchdog_timeout_ms.max(1);
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

    t
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
    fn load_defaults_when_missing() {
        let dir = tempdir().unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        assert!(cfg.get_paths("music").is_empty());
    }

    #[test]
    fn add_remove_round_trips_to_disk() {
        let dir = tempdir().unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        let added = cfg.add_path("music", dir.path().to_str().unwrap()).unwrap();
        assert!(added);
        // re-add returns false (already present)
        let again = cfg.add_path("music", dir.path().to_str().unwrap()).unwrap();
        assert!(!again);

        // reopen reads persisted state
        drop(cfg);
        let cfg2 = Config::open(dir.path()).unwrap();
        assert_eq!(cfg2.get_paths("music").len(), 1);

        let removed = cfg2
            .remove_path("music", dir.path().to_str().unwrap())
            .unwrap();
        assert!(removed);
        assert!(cfg2.get_paths("music").is_empty());
    }

    #[test]
    fn migrates_legacy_library_paths_field() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"libraryPaths": ["/old"]}"#).unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        assert_eq!(cfg.get_paths("music"), vec!["/old".to_string()]);
    }

    #[test]
    fn unknown_kind_is_noop() {
        let dir = tempdir().unwrap();
        let cfg = Config::open(dir.path()).unwrap();
        assert!(!cfg.add_path("bogus", "/x").unwrap());
        assert!(cfg.get_paths("bogus").is_empty());
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
        })
        .unwrap();
        drop(cfg);

        let reopened = Config::open(dir.path()).unwrap();
        let appearance = reopened.get_appearance();
        assert_eq!(appearance.theme_id, "station-red");
        assert_eq!(appearance.station_name.as_deref(), Some("Radio Foo"));
        assert_eq!(appearance.logo.as_deref(), Some("logo.svg"));
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
        let clamped = cfg.set_tuning(t).unwrap();
        assert_eq!(clamped.interleave.jingle_every, 0);
        assert_eq!(clamped.interleave.commercial_every, 0);
        assert_eq!(clamped.auto_playlist.auto_playlist_threshold, 4);
        assert_eq!(clamped.cache.max_cache_bytes, 16 * 1024 * 1024);
        assert_eq!(clamped.player.read_retry_backoffs_ms, vec![1, 5]);

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
