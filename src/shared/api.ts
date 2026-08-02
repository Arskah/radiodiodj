import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import type {
  ContentType,
  DeviceInfo,
  DeviceRef,
  LibraryStats,
  NowPlayingConfig,
  ScanResult,
  SortColumn,
  SortDir,
  Track,
  TrackMetadataInput,
} from "./types";

export type PersistedPlaylistItem =
  { kind: "track"; id: number } | { kind: "stop" };

export interface SessionPersistState {
  playlistIds: number[];
  playlistItems: PersistedPlaylistItem[];
  historyIds: number[];
  currentTrackId: number | null;
  currentTime: number;
  autoPlaylistActive: boolean;
  autoAdvance: boolean;
  volume: number;
  cueVolume: number;
}

export interface SessionLoadResult {
  state: SessionPersistState;
  tracks: Track[];
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
  prefetch(ids: number[]): Promise<void> {
    return invoke<void>("main_deck_prefetch", { ids });
  },
  generatePlaylist(count: number, excludeIds: number[]): Promise<Track[]> {
    return invoke<Track[]>("generate_playlist", { count, excludeIds });
  },
  pickFiller(contentType: ContentType): Promise<Track | null> {
    return invoke<Track | null>("pick_filler", { contentType });
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
  testNowPlayingWebhook(): Promise<number> {
    return invoke<number>("now_playing_test");
  },
  broadcastShutdown(): Promise<void> {
    return invoke<void>("broadcast_shutdown");
  },
  updateTrackMetadata(updates: TrackMetadataInput): Promise<Track> {
    const filtered: Partial<TrackMetadataInput & { id: number }> = {
      id: updates.id,
    };
    if (updates.title !== "") {
      filtered.title = updates.title;
    }
    if (updates.artist !== "") {
      filtered.artist = updates.artist;
    }
    if (updates.album !== "") {
      filtered.album = updates.album;
    }
    const genreVal = updates.genre;
    if (genreVal != null) {
      filtered.genre = genreVal;
    }
    const yearVal = updates.year;
    if (yearVal != null) {
      filtered.year = yearVal;
    }
    return invoke<Track>("update_track_metadata", { updates: filtered });
  },
  async pickDirectory(): Promise<string | null> {
    const dir = await open({ directory: true, multiple: false });
    return typeof dir === "string" ? dir : null;
  },
};

export type Api = typeof api;
