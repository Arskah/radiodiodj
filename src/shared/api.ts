import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import type {
  ContentType,
  CuePoints,
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
  historyIds: number[];
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
  /** The track that just left the main deck. The renderer appends it to history. */
  displaced: Track | null;
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
  /** Step back to a track taken off the renderer's own history. */
  playlistPrev(id: number): Promise<void> {
    return invoke<void>("playlist_prev", { id });
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
  async pickDirectory(): Promise<string | null> {
    const dir = await open({ directory: true, multiple: false });
    return typeof dir === "string" ? dir : null;
  },
};

export type Api = typeof api;
