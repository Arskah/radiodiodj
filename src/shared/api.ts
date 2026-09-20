import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import type {
  AdminStatus,
  Appearance,
  ContentType,
  CuePoints,
  DeckRoleEntry,
  PlaylistItem,
  DeviceInfo,
  DeviceRef,
  FindingKind,
  HealthReport,
  LibraryStats,
  NowPlayingConfig,
  ScanResult,
  SortColumn,
  SortDir,
  ImageSlot,
  ThemeListing,
  Track,
  TrackMetadataInput,
  TuningConfig,
} from "./types";

export type PersistedPlaylistItem =
  | { kind: "track"; id: number; cue_override: CuePoints | null }
  | { kind: "stop" };

export interface SessionPersistState {
  playlistIds: number[];
  playlistItems: PersistedPlaylistItem[];
  currentTrackId: number | null;
  currentTime: number;
  /** The override the track on air is playing under, if any. */
  currentCueOverride: CuePoints | null;
  autoPlaylistActive: boolean;
  autoAdvance: boolean;
  volume: number;
  cueVolume: number;
}

export interface SessionLoadResult {
  state: SessionPersistState;
  tracks: Track[];
  /** This launch replaced a library database from an older version. */
  libraryReset: boolean;
}

/**
 * Whole-playlist snapshot from the backend, which owns the playlist. Arrives on
 * every mutation and every advance; the renderer mirrors it rather than keeping
 * a playlist of its own. See `docs/backend-owned-playlist.md`.
 */
export interface PlaylistSnapshot {
  playlist: PlaylistItem[];
  current: Track | null;
  /** What has aired, oldest first, capped by the stored `historyCap`. */
  history: Track[];
  autoPlaylistActive: boolean;
  autoAdvance: boolean;
  /** The override the track on air came on air under, if it came off an item that carried one. */
  currentOverride: CuePoints | null;
  /** Playback is blocked waiting for the media share. Drives the reconnecting banner. */
  awaitingNetwork: boolean;
}

export type ScanStatus =
  | { status: "idle"; lastResult: ScanResult | null }
  | { status: "running"; processed: number; total: number }
  | { status: "canceled"; processed: number; total: number; added: number }
  | { status: "error"; message: string };

export interface ScanProgress {
  processed: number;
  total: number;
}

export type WaveformStatus =
  { status: "idle" } | { status: "running"; processed: number; total: number };

export const api = {
  search(
    query: string,
    contentType?: ContentType,
    sortBy?: SortColumn,
    sortDir?: SortDir,
  ): Promise<Track[]> {
    return invoke<Track[]>("search", { query, contentType, sortBy, sortDir });
  },
  getTrack(id: number): Promise<Track> {
    return invoke<Track>("get_track", { id });
  },
  getTracksByIds(ids: number[]): Promise<Track[]> {
    return invoke<Track[]>("get_tracks_by_ids", { ids });
  },
  /**
   * Fetch a track's amplitude-curve peaks (one byte per bucket, 0..=255) for
   * the seek UI, or `null` when the track has no stored waveform.
   */
  getWaveform(id: number): Promise<number[] | null> {
    return invoke<number[] | null>("get_waveform", { id });
  },
  /**
   * Decode a track into the cue editor's fine curve: one byte (0..=255) per
   * 10 ms of audio. Computed on demand, so this can take a few seconds.
   */
  async getWaveformDetail(id: number): Promise<Uint8Array> {
    const buf = await invoke<ArrayBuffer>("get_waveform_detail", { id });
    return new Uint8Array(buf);
  },
  /**
   * Fetch a track's embedded cover art as a base64 `data:` URL for the deck's
   * vinyl disc, or `null` when the file has no artwork. Read on demand.
   */
  getCoverArt(id: number): Promise<string | null> {
    return invoke<string | null>("get_cover_art", { id });
  },
  loadSession(): Promise<SessionLoadResult> {
    return invoke<SessionLoadResult>("load_session");
  },
  saveSession(state: SessionPersistState): Promise<void> {
    return invoke<void>("save_session", { state });
  },
  trackPlayed(id: number): Promise<void> {
    return invoke<void>("track_played", { id });
  },
  /**
   * Whether the main deck is playing right now. `pause-state` is an event, so a
   * window that attaches late — a reload, or a session the backend restored
   * before this window existed — has to ask rather than assume.
   */
  mainDeckIsPlaying(): Promise<boolean> {
    return invoke<boolean>("main_deck_is_playing");
  },
  /**
   * Ramp the on-air deck to silence and stop it. The playlist stays where it
   * is, as it does for Stop. Omit `ms` to use the configured duration.
   */
  mainDeckFadeOut(ms?: number): Promise<void> {
    return invoke<void>("main_deck_fade_out", { ms: ms ?? null });
  },
  /**
   * Start the next item now and fade the outgoing track out underneath it.
   * With nothing armed to overlap with, the fade ends the track instead and
   * the playlist advances as it would at any other end of track.
   */
  mainDeckFadeToNext(ms?: number): Promise<void> {
    return invoke<void>("main_deck_fade_to_next", { ms: ms ?? null });
  },
  /** Current playlist state, for a renderer that has just started up. */
  playlistSync(): Promise<PlaylistSnapshot> {
    return invoke<PlaylistSnapshot>("playlist_sync");
  },
  onPlaylistState(
    callback: (snapshot: PlaylistSnapshot) => void,
  ): Promise<UnlistenFn> {
    return listen<PlaylistSnapshot>("program:playlist-state", (e) =>
      callback(e.payload),
    );
  },
  /**
   * Which program deck holds which role, and what is on it. The transport and
   * Now playing speak role-mapped `main-deck:*` events instead; this is what
   * says a handover has left an outgoing track playing out underneath.
   */
  onDeckRoles(callback: (roles: DeckRoleEntry[]) => void): Promise<UnlistenFn> {
    return listen<DeckRoleEntry[]>("program:roles", (e) => callback(e.payload));
  },
  /** Air position of the outgoing track during an overlap. */
  onTailTime(callback: (seconds: number) => void): Promise<UnlistenFn> {
    return listen<number>("tail-deck:time", (e) => callback(e.payload));
  },
  /** Air duration of the outgoing track during an overlap. */
  onTailDuration(callback: (seconds: number) => void): Promise<UnlistenFn> {
    return listen<number>("tail-deck:duration", (e) => callback(e.payload));
  },
  /** Show the track's file in Finder / Explorer / the file manager. */
  revealTrack(id: number): Promise<void> {
    return invoke<void>("reveal_track", { id });
  },
  playlistAdd(id: number): Promise<void> {
    return invoke<void>("playlist_add", { id });
  },
  /**
   * Insert at the head as next-up — cue promotion. `cuePoints` overrides the
   * track's radio edit for that one airing; `null` references the track.
   */
  playlistAddFront(
    id: number,
    cuePoints: CuePoints | null = null,
  ): Promise<void> {
    return invoke<void>("playlist_add_front", { id, cuePoints });
  },
  /** Set (or, with `null`, clear) one queued item's cue-point override. */
  playlistSetItemCuePoints(
    index: number,
    cuePoints: CuePoints | null,
  ): Promise<void> {
    return invoke<void>("playlist_set_item_cue_points", { index, cuePoints });
  },
  playlistAddStopMarker(): Promise<void> {
    return invoke<void>("playlist_add_stop_marker");
  },
  playlistAddFiller(contentType: ContentType): Promise<void> {
    return invoke<void>("playlist_add_filler", { contentType });
  },
  playlistRemove(index: number): Promise<void> {
    return invoke<void>("playlist_remove", { index });
  },
  playlistMove(from: number, to: number): Promise<void> {
    return invoke<void>("playlist_move", { from, to });
  },
  playlistClear(): Promise<void> {
    return invoke<void>("playlist_clear");
  },
  playlistPlayIndex(index: number): Promise<void> {
    return invoke<void>("playlist_play_index", { index });
  },
  playlistPlayNow(id: number): Promise<void> {
    return invoke<void>("playlist_play_now", { id });
  },
  playlistNext(): Promise<void> {
    return invoke<void>("playlist_next");
  },
  /** Step back to the last track that aired. */
  playlistPrev(): Promise<void> {
    return invoke<void>("playlist_prev");
  },
  playlistStop(): Promise<void> {
    return invoke<void>("playlist_stop");
  },
  playlistSetAutoPlaylist(active: boolean): Promise<void> {
    return invoke<void>("playlist_set_auto_playlist", { active });
  },
  playlistSetAutoAdvance(active: boolean): Promise<void> {
    return invoke<void>("playlist_set_auto_advance", { active });
  },
  getStats(): Promise<LibraryStats> {
    return invoke<LibraryStats>("get_stats");
  },
  getPaths(type: ContentType): Promise<string[]> {
    return invoke<string[]>("get_paths", { type });
  },
  getAllPaths(): Promise<Record<ContentType, string[]>> {
    return invoke<Record<ContentType, string[]>>("get_all_paths");
  },
  async addPath(type: ContentType): Promise<string | null> {
    const dir = await open({ directory: true, multiple: false });
    if (typeof dir !== "string") return null;
    const ok = await invoke<boolean>("add_path", { type, dirPath: dir });
    return ok ? dir : null;
  },
  removePath(type: ContentType, dirPath: string): Promise<boolean> {
    return invoke<boolean>("remove_path", { type, dirPath });
  },
  /**
   * Permanently delete the given missing tracks. Ids of tracks that are not
   * missing are ignored; resolves to how many were deleted.
   */
  purgeTracks(ids: number[]): Promise<number> {
    return invoke<number>("purge_tracks", { ids });
  },
  libraryHealth(): Promise<HealthReport> {
    return invoke<HealthReport>("library_health");
  },
  onLibraryHealth(
    callback: (report: HealthReport) => void,
  ): Promise<UnlistenFn> {
    return listen<HealthReport>("library-health", (e) => callback(e.payload));
  },
  /** Compare the disk with the library now, outside the timer. */
  libraryCheckNow(): Promise<void> {
    return invoke<void>("library_check_now");
  },
  healthDismiss(kind: FindingKind, key = ""): Promise<void> {
    return invoke<void>("health_dismiss", { kind, key });
  },
  healthUndismiss(kind: FindingKind, key = ""): Promise<void> {
    return invoke<void>("health_undismiss", { kind, key });
  },
  scanLibraries(): Promise<{ alreadyRunning: boolean }> {
    return invoke<{ alreadyRunning: boolean }>("scan_libraries");
  },
  cancelScan(): Promise<void> {
    return invoke<void>("cancel_scan");
  },
  getScanStatus(): Promise<ScanStatus> {
    return invoke<ScanStatus>("get_scan_status");
  },
  onScanProgress(callback: (data: ScanProgress) => void): Promise<UnlistenFn> {
    return listen<ScanProgress>("scan-progress", (e) => callback(e.payload));
  },
  onScanStateChanged(
    callback: (data: ScanStatus) => void,
  ): Promise<UnlistenFn> {
    return listen<ScanStatus>("scan-state-changed", (e) => callback(e.payload));
  },
  /**
   * Fires when the background worker has stored a track's waveform. Payload is
   * the track id; the renderer refetches the curve if that track is loaded.
   */
  onWaveformReady(callback: (id: number) => void): Promise<UnlistenFn> {
    return listen<number>("waveform-ready", (e) => callback(e.payload));
  },
  /**
   * Fires when the background worker has derived a track's cue points. Every
   * copy of the track the UI holds takes the new markers, since durations are
   * derived from them.
   */
  onCuePointsReady(
    callback: (id: number, points: CuePoints) => void,
  ): Promise<UnlistenFn> {
    return listen<{ id: number; cuePoints: CuePoints }>(
      "cue-points-ready",
      (e) => callback(e.payload.id, e.payload.cuePoints),
    );
  },
  getWaveformStatus(): Promise<WaveformStatus> {
    return invoke<WaveformStatus>("get_waveform_status");
  },
  onWaveformProgress(
    callback: (data: ScanProgress) => void,
  ): Promise<UnlistenFn> {
    return listen<ScanProgress>("waveform-progress", (e) =>
      callback(e.payload),
    );
  },
  onWaveformStateChanged(
    callback: (data: WaveformStatus) => void,
  ): Promise<UnlistenFn> {
    return listen<WaveformStatus>("waveform-state-changed", (e) =>
      callback(e.payload),
    );
  },
  listAudioDevices(): Promise<DeviceInfo[]> {
    return invoke<DeviceInfo[]>("audio_list_devices");
  },
  getMainDevice(): Promise<DeviceRef | null> {
    return invoke<DeviceRef | null>("get_main_device");
  },
  setMainDevice(device: DeviceRef | null): Promise<void> {
    return invoke<void>("set_main_device", { device });
  },
  getCueDevice(): Promise<DeviceRef | null> {
    return invoke<DeviceRef | null>("get_cue_device");
  },
  setCueDevice(device: DeviceRef | null): Promise<void> {
    return invoke<void>("set_cue_device", { device });
  },
  getNowPlayingConfig(): Promise<NowPlayingConfig> {
    return invoke<NowPlayingConfig>("get_now_playing_config");
  },
  setNowPlayingConfig(config: NowPlayingConfig): Promise<void> {
    return invoke<void>("set_now_playing_config", { config });
  },
  getTuningConfig(): Promise<TuningConfig> {
    return invoke<TuningConfig>("get_tuning_config");
  },
  // Returns the clamped config the backend actually persisted, so the UI can
  // reflect any values that were coerced into range.
  setTuningConfig(config: TuningConfig): Promise<TuningConfig> {
    return invoke<TuningConfig>("set_tuning_config", { config });
  },
  testNowPlayingWebhook(): Promise<number> {
    return invoke<number>("now_playing_test");
  },
  broadcastShutdown(): Promise<void> {
    return invoke<void>("broadcast_shutdown");
  },
  updateTrackMetadata(updates: TrackMetadataInput): Promise<Track> {
    // Partial patch (RFC 7396 style): forward only the keys the caller set.
    // Absent key → leave unchanged; present value → set (empty string
    // included); present `null` → clear the column (a present JSON `null`
    // deserializes to `Some(None)` on the backend). Undefined keys are dropped
    // explicitly rather than relying on the serializer to omit them.
    const payload: TrackMetadataInput = { id: updates.id };
    if (updates.title !== undefined) payload.title = updates.title;
    if (updates.artist !== undefined) payload.artist = updates.artist;
    if (updates.album !== undefined) payload.album = updates.album;
    if (updates.genre !== undefined) payload.genre = updates.genre;
    if (updates.year !== undefined) payload.year = updates.year;
    return invoke<Track>("update_track_metadata", { updates: payload });
  },
  revertTrackTags(id: number): Promise<Track> {
    return invoke<Track>("revert_track_tags", { id });
  },
  retryTagWrite(id: number): Promise<void> {
    return invoke<void>("retry_tag_write", { id });
  },
  dismissTagWrite(id: number): Promise<void> {
    return invoke<void>("dismiss_tag_write", { id });
  },
  // Returns the clamped points the backend actually stored, so the UI reflects
  // any marker that was coerced into order or inside the file.
  setCuePoints(id: number, points: CuePoints): Promise<CuePoints> {
    return invoke<CuePoints>("set_cue_points", { id, points });
  },
  adminStatus(): Promise<AdminStatus> {
    return invoke<AdminStatus>("admin_status");
  },
  /** Resolves to whether the password matched. */
  adminUnlock(password: string): Promise<boolean> {
    return invoke<boolean>("admin_unlock", { password });
  },
  adminLock(): Promise<void> {
    return invoke<void>("admin_lock");
  },
  adminSetPassword(password: string): Promise<void> {
    return invoke<void>("admin_set_password", { password });
  },
  adminClearPassword(): Promise<void> {
    return invoke<void>("admin_clear_password");
  },
  adminSetIdleLockMin(minutes: number): Promise<void> {
    return invoke<void>("admin_set_idle_lock_min", { minutes });
  },
  onAdminStateChanged(
    callback: (status: AdminStatus) => void,
  ): Promise<UnlistenFn> {
    return listen<AdminStatus>("admin-state-changed", (e) =>
      callback(e.payload),
    );
  },
  /**
   * The resolved appearance to paint. Called before the app mounts, so it must
   * stay ungated — a launch starts locked.
   */
  getAppearance(): Promise<Appearance> {
    return invoke<Appearance>("get_appearance");
  },
  listThemes(): Promise<ThemeListing[]> {
    return invoke<ThemeListing[]>("list_themes");
  },
  /** Returns the resolved appearance the backend actually applied. */
  setTheme(themeId: string): Promise<Appearance> {
    return invoke<Appearance>("set_theme", { themeId });
  },
  /** Re-read the themes directory and re-resolve the active theme. */
  reloadThemes(): Promise<Appearance> {
    return invoke<Appearance>("reload_themes");
  },
  revealThemesDir(): Promise<void> {
    return invoke<void>("reveal_themes_dir");
  },
  /** Returns the stored name, which the backend trimmed and capped. */
  setStationName(name: string | null): Promise<Appearance> {
    return invoke<Appearance>("set_station_name", { name });
  },
  /** Copies the chosen file into the app's own branding directory. */
  setStationImage(slot: ImageSlot, path: string): Promise<Appearance> {
    return invoke<Appearance>("set_station_image", { slot, path });
  },
  clearStationImage(slot: ImageSlot): Promise<Appearance> {
    return invoke<Appearance>("clear_station_image", { slot });
  },
  /** Pick an image file. The app copies it, so the chosen path is not kept. */
  async pickImageFile(): Promise<string | null> {
    const file = await open({
      multiple: false,
      filters: [
        { name: "Images", extensions: ["svg", "png", "jpg", "jpeg", "webp"] },
      ],
    });
    return typeof file === "string" ? file : null;
  },
  async pickDirectory(): Promise<string | null> {
    const dir = await open({ directory: true, multiple: false });
    return typeof dir === "string" ? dir : null;
  },
};

export type Api = typeof api;
