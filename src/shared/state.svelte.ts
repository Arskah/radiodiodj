import type {
  ContentType,
  DeviceInfo,
  DeviceRef,
  LibraryStats,
  PlaylistItem,
  SortColumn,
  SortDir,
  Track,
} from "./types";
import { isStopMarker, isTrackItem, stopMarker, trackItem } from "./types";
import {
  api,
  type PersistedPlaylistItem,
  type ScanStatus,
  type SessionLoadResult,
  type WaveformStatus,
} from "./api";
import type { DeckBackend } from "../features/deck/backend";
import { NativeBackend } from "../features/deck/nativeBackend";
import { throttle, type Throttled } from "./throttle";
import { isStrictNever } from "./isStrictNever";
import { APP_NAME } from "./appName";

const logger = {
  error: (...args: unknown[]) => console.error(...args),
};

export type { Track };

// Auto playlist configuration
const AUTO_PLAYLIST_BUFFER = 20;
// Threshold at which the auto playlist will be refilled. Should be lower than AUTO_PLAYLIST_BUFFER to avoid excessive refilling.
const AUTO_PLAYLIST_THRESHOLD = 5;
const HISTORY_CAP = 100;
const SESSION_SAVE_THROTTLE_MS = 500;
// Backoff schedule (ms) for retrying advancement while nothing playable is
// cached (network outage). The last value repeats until recovery.
const NET_RETRY_BACKOFFS_MS = [1000, 2000, 5000];

export type PlaylistTab = "playlist" | "history";

export class AppState {
  searchQuery = $state("");
  activeTab = $state<ContentType>("music");
  playlistTab = $state<PlaylistTab>("playlist");
  sortBy = $state<SortColumn | null>(null);
  sortDir = $state<SortDir>("asc");
  tracks = $state<Track[]>([]);
  stats = $state<LibraryStats | null>(null);
  libraryPaths = $state<Record<ContentType, string[]>>({
    music: [],
    commercial: [],
    jingle: [],
  });
  settingsOpen = $state(false);
  scanStatus = $state<ScanStatus>({ status: "idle", lastResult: null });
  // Progress of the background waveform pass (runs after the metadata scan).
  waveformStatus = $state<WaveformStatus>({ status: "idle" });

  playlist = $state<PlaylistItem[]>([]);
  history = $state<Track[]>([]);
  currentTrack = $state<Track | null>(null);
  autoPlaylistActive = $state(false);
  autoAdvance = $state(true);
  isPlaying = $state(false);
  isBuffering = $state(false);
  volume = $state(1);
  currentTime = $state(0);
  duration = $state(0);
  // Amplitude-curve peaks (0..=255 per bucket) for the current main-deck track,
  // rendered behind the seek bar. `null` while none is loaded or the track has
  // no stored waveform (falls back to a plain progress bar).
  waveform = $state<number[] | null>(null);
  // Base64 `data:` URL of the current main-deck track's embedded cover art,
  // shown on the spinning vinyl disc. `null` while none is loaded or the track
  // has no artwork (the disc falls back to a note-icon placeholder).
  coverArt = $state<string | null>(null);
  // Track ids currently resident in the backend prefetch cache. Drives
  // skip-to-cached advancement during a network outage; membership is updated
  // by cache-state events.
  cachedIds = $state<Set<number>>(new Set());
  // True while no upcoming track is cached and we are waiting for the share to
  // recover. Set by the playback-advancement state machine when playback is
  // actually blocked. Cleared on resume.
  awaitingNetwork = $state(false);
  // True while a prefetch read is failing — the share looks unreachable even
  // if the current in-RAM track keeps playing. Set by prefetch-failed events,
  // cleared by the next successful cache-state (a read succeeded).
  shareUnreachable = $state(false);
  // True while the main deck cannot open an audio output device (none present,
  // or the configured one won't open). The backend auto-retries every 2s and
  // clears this on recovery. Distinct from a network outage: the device, not
  // the media share, is the problem. See issue #259.
  outputUnavailable = $state(false);

  // Cue deck (independent transport on a separate audio device)
  cueTrack = $state<Track | null>(null);
  cueIsPlaying = $state(false);
  cueIsBuffering = $state(false);
  cueCurrentTime = $state(0);
  cueDuration = $state(0);
  cueVolume = $state(1);
  cueError = $state<string | null>(null);
  // True while the cue deck cannot open its audio output device (see
  // `outputUnavailable` for the main deck). Backend auto-retries and clears it.
  cueOutputUnavailable = $state(false);
  // Amplitude-curve peaks for the current cue-deck track (see `waveform`).
  cueWaveform = $state<number[] | null>(null);
  // Cover-art data URL for the current cue-deck track (see `coverArt`).
  cueCoverArt = $state<string | null>(null);

  // Audio device config
  audioDevices = $state<DeviceInfo[]>([]);
  mainDevice = $state<DeviceRef | null>(null);
  cueDevice = $state<DeviceRef | null>(null);

  hoveredTrack = $state<Track | null>(null);
  hoverX = $state(0);
  hoverY = $state(0);

  // Track currently being edited via the MetadataEditor overlay.
  editingTrack = $state<Track | null>(null);

  backend: DeckBackend;
  cueBackend: DeckBackend;

  private throttledSave: Throttled;
  private sessionLoaded = false;
  private netRetryTimer: ReturnType<typeof setTimeout> | null = null;
  private netRetryAttempt = 0;

  constructor(backend?: DeckBackend, cueBackend?: DeckBackend) {
    this.backend = backend ?? new NativeBackend("main");
    this.cueBackend = cueBackend ?? new NativeBackend("cue");
    this.throttledSave = throttle(
      () => void this.persistSession(),
      SESSION_SAVE_THROTTLE_MS,
    );

    this.backend.on((event) => {
      switch (event.type) {
        case "pause-state":
          this.isPlaying = !event.paused;
          break;
        case "time":
          this.currentTime = event.seconds;
          this.scheduleSave();
          break;
        case "duration":
          this.duration = event.seconds;
          break;
        case "ended":
          void this.handleEnded();
          break;
        case "buffering":
          this.isBuffering = event.buffering;
          break;
        case "cache-state":
          this.cachedIds = new Set(event.ids);
          // A read succeeded, so the share is reachable again.
          this.shareUnreachable = false;
          // Cache membership changed — if we were stalled waiting for the
          // share, a newly-cached track may now be playable.
          if (this.awaitingNetwork) this.advancePlan(true);
          break;
        case "prefetch-failed":
          // A prefetch read failed — flag the share as unreachable so the UI
          // can warn even while an in-RAM track keeps playing.
          this.shareUnreachable = true;
          break;
        case "output-unavailable":
          // No audio device could be opened (or it recovered). The backend
          // auto-retries; the banner shows until a device is available.
          this.outputUnavailable = event.unavailable;
          break;
        case "error":
          this.isBuffering = false;
          logger.error("Audio error:", event.message);
          break;
        case "load-failed":
          // Read failed after retries or watchdog timeout. Skip to the next
          // cached track (or wait for the share) instead of dead air.
          this.isBuffering = false;
          logger.error("Audio load failed for track:", event.id);
          if (this.autoAdvance) this.advancePlan(true);
          break;
        default:
          return isStrictNever(event);
      }
    });

    this.cueBackend.on((event) => {
      switch (event.type) {
        case "pause-state":
          this.cueIsPlaying = !event.paused;
          break;
        case "time":
          this.cueCurrentTime = event.seconds;
          break;
        case "duration":
          this.cueDuration = event.seconds;
          break;
        case "ended":
          this.cueIsPlaying = false;
          this.cueCurrentTime = 0;
          break;
        case "buffering":
          this.cueIsBuffering = event.buffering;
          break;
        case "error":
          this.cueIsBuffering = false;
          logger.error("Cue audio error:", event.message);
          this.cueError = event.message;
          break;
        case "load-failed":
          // Read failed after retries or watchdog timeout. Clear buffering;
          // skip-to-cached handling is a later issue.
          this.cueIsBuffering = false;
          logger.error("Cue audio load failed for track:", event.id);
          break;
        case "output-unavailable":
          this.cueOutputUnavailable = event.unavailable;
          break;
        case "cache-state":
        case "prefetch-failed":
          break; // cue deck doesn't use the cache, ignore
        default:
          return isStrictNever(event);
      }
    });

    api.onScanProgress(({ processed, total }) => {
      if (this.scanStatus.status === "running") {
        this.scanStatus = { status: "running", processed, total };
      }
    });

    api.onScanStateChanged((next) => {
      const wasRunning = this.scanStatus.status === "running";
      this.scanStatus = next;
      if (wasRunning && next.status !== "running") {
        void this.search();
        void this.loadStats();
      }
    });

    // A waveform the background worker just computed may belong to a track that
    // was already loaded (its earlier fetch came back empty). Refetch so the
    // seek bar fills in without a reload.
    api.onWaveformReady((id) => {
      if (this.currentTrack?.id === id) this.loadWaveform(id);
      if (this.cueTrack?.id === id) this.loadCueWaveform(id);
    });

    api.onWaveformProgress(({ processed, total }) => {
      if (this.waveformStatus.status === "running") {
        this.waveformStatus = { status: "running", processed, total };
      }
    });
    api.onWaveformStateChanged((next) => {
      this.waveformStatus = next;
    });
  }

  get progressPct(): number {
    return this.duration ? (this.currentTime / this.duration) * 100 : 0;
  }

  // Drives the "Reconnecting…" banner: playback is blocked waiting for the
  // share, or a prefetch read is currently failing.
  get reconnecting(): boolean {
    return this.awaitingNetwork || this.shareUnreachable;
  }

  get cueProgressPct(): number {
    return this.cueDuration
      ? (this.cueCurrentTime / this.cueDuration) * 100
      : 0;
  }

  setVolume(v: number): void {
    this.volume = v;
    void this.backend.setVolume(v);
    this.scheduleSave();
  }

  async search(): Promise<void> {
    this.tracks = await api.search(
      this.searchQuery,
      this.activeTab,
      this.sortBy ?? undefined,
      this.sortDir,
    );
  }

  setTab(tab: ContentType): void {
    this.activeTab = tab;
    void this.search();
  }

  toggleSort(column: SortColumn): void {
    if (this.sortBy === column) {
      this.sortDir = this.sortDir === "asc" ? "desc" : "asc";
    } else {
      this.sortBy = column;
      this.sortDir = "asc";
    }
    void this.search();
  }

  addToPlaylist(track: Track): void {
    this.playlist.push(trackItem(track));
    this.updatePrefetch();
    this.scheduleSave();
  }

  addStopMarker(): void {
    this.playlist.push(stopMarker());
    this.updatePrefetch();
    this.scheduleSave();
  }

  async addFiller(contentType: ContentType): Promise<void> {
    const track = await api.pickFiller(contentType);
    if (!track) return;
    this.playlist.push(trackItem(track));
    this.updatePrefetch();
    this.scheduleSave();
  }

  playNow(track: Track): void {
    this.playTrack(track);
  }

  removeFromPlaylist(index: number): void {
    this.playlist.splice(index, 1);
    this.updatePrefetch();
    this.scheduleSave();
  }

  movePlaylistItem(from: number, to: number): void {
    if (from === to) return;
    const [item] = this.playlist.splice(from, 1);
    this.playlist.splice(to, 0, item);
    this.updatePrefetch();
    this.scheduleSave();
  }

  clearPlaylist(): void {
    this.playlist.length = 0;
    this.updatePrefetch();
    this.scheduleSave();
  }

  playIndex(index: number): void {
    if (index < 0 || index >= this.playlist.length) return;
    const [item] = this.playlist.splice(index, 1);
    if (isStopMarker(item)) {
      this.stop();
      return;
    }
    this.playTrack(item.track);
  }

  private playTrack(track: Track): void {
    if (this.currentTrack) {
      this.appendHistory(this.currentTrack);
    }
    this.setCurrent(track);
  }

  get historyDisplay(): Track[] {
    return this.history.slice().reverse();
  }

  appendHistory(track: Track): void {
    this.history.push(track);
    if (this.history.length > HISTORY_CAP) {
      this.history.splice(0, this.history.length - HISTORY_CAP);
    }
    this.scheduleSave();
  }

  removeFromHistory(displayIndex: number): void {
    const i = this.history.length - 1 - displayIndex;
    if (i < 0 || i >= this.history.length) return;
    this.history.splice(i, 1);
    this.scheduleSave();
  }

  clearHistory(): void {
    this.history.length = 0;
    this.scheduleSave();
  }

  requeueFromHistory(displayIndex: number): void {
    const i = this.history.length - 1 - displayIndex;
    const track = this.history[i];
    if (!track) return;
    this.playlist.push(trackItem(track));
    this.updatePrefetch();
    this.scheduleSave();
  }

  private setCurrent(track: Track): void {
    // Any explicit track change (manual next/prev, playIndex, session restore)
    // supersedes a pending outage retry. Without this, an armed retry timer —
    // or a cache-state arriving while awaitingNetwork is still set — fires after
    // the new track loads and advances again, skipping it.
    this.clearNetRetry();
    this.currentTrack = track;
    this.duration = track.duration ?? 0;
    this.currentTime = 0;
    this.loadWaveform(track.id);
    this.loadCoverArt(track.id);
    void this.loadAndPlay(track);
    void api.trackPlayed(track.id);
    document.title = `${track.title} - ${track.artist} | ${APP_NAME}`;
    void this.maybeRefillPlaylist();
    this.updatePrefetch();
    this.scheduleSave();
  }

  private async loadAndPlay(track: Track): Promise<void> {
    try {
      await this.backend.load(track.id);
      await this.backend.play();
    } catch (err) {
      logger.error("Load/play failed:", err);
    }
  }

  /**
   * Fetch the amplitude curve for `id` and store it, guarding against a race:
   * a slower fetch for a track the user has already skipped past must not
   * overwrite the current one. The result is dropped unless `id` is still the
   * loaded track when it arrives.
   */
  private loadWaveform(id: number): void {
    this.waveform = null;
    void api
      .getWaveform(id)
      .then((peaks) => {
        if (this.currentTrack?.id === id) this.waveform = peaks;
      })
      .catch((err) => logger.error("Waveform load failed:", err));
  }

  private loadCueWaveform(id: number): void {
    this.cueWaveform = null;
    void api
      .getWaveform(id)
      .then((peaks) => {
        if (this.cueTrack?.id === id) this.cueWaveform = peaks;
      })
      .catch((err) => logger.error("Cue waveform load failed:", err));
  }

  /**
   * Fetch the current main-deck track's cover art, with the same race guard as
   * `loadWaveform`: a slow fetch for a track the user has skipped past must not
   * overwrite the art now showing.
   */
  private loadCoverArt(id: number): void {
    this.coverArt = null;
    void api
      .getCoverArt(id)
      .then((art) => {
        if (this.currentTrack?.id === id) this.coverArt = art;
      })
      .catch((err) => logger.error("Cover art load failed:", err));
  }

  private loadCueCoverArt(id: number): void {
    this.cueCoverArt = null;
    void api
      .getCoverArt(id)
      .then((art) => {
        if (this.cueTrack?.id === id) this.cueCoverArt = art;
      })
      .catch((err) => logger.error("Cue cover art load failed:", err));
  }

  togglePlay(): void {
    if (!this.currentTrack) {
      if (this.playlist.length > 0) this.playIndex(0);
      return;
    }
    if (this.isPlaying) {
      void this.backend.pause();
    } else {
      void this.backend
        .play()
        .catch((err) => logger.error("Resume failed:", err));
    }
  }

  stop(): void {
    this.clearNetRetry();
    if (this.currentTrack) this.appendHistory(this.currentTrack);
    void this.backend.stop();
    this.currentTrack = null;
    this.autoPlaylistActive = false;
    this.currentTime = 0;
    this.duration = 0;
    this.waveform = null;
    this.coverArt = null;
    this.isPlaying = false;
    document.title = APP_NAME;
    this.updatePrefetch();
    this.scheduleSave();
  }

  next(): void {
    if (this.playlist.length > 0) this.playIndex(0);
  }

  prev(): void {
    if (this.currentTrack && this.currentTime > 3) {
      this.currentTime = 0;
      void this.backend.seek(0);
      return;
    }
    const previous = this.history[this.history.length - 1];
    if (!previous) {
      if (this.currentTrack) {
        this.currentTime = 0;
        void this.backend.seek(0);
      }
      return;
    }
    if (this.currentTrack?.id === previous.id) {
      this.currentTime = 0;
      void this.backend.seek(0);
      return;
    }
    if (this.currentTrack) this.playlist.unshift(trackItem(this.currentTrack));
    this.setCurrent(previous);
  }

  toggleMode(): void {
    this.autoAdvance = !this.autoAdvance;
    this.scheduleSave();
  }

  async toggleAutoPlaylist(): Promise<void> {
    this.autoPlaylistActive = !this.autoPlaylistActive;
    if (this.autoPlaylistActive) {
      await this.maybeRefillPlaylist();
      if (!this.currentTrack && this.playlist.length > 0) {
        this.playIndex(0);
      }
    }
    this.scheduleSave();
  }

  async maybeRefillPlaylist(): Promise<void> {
    if (!this.autoPlaylistActive) return;
    if (this.playlist.some(isStopMarker)) return;
    if (this.playlist.length < AUTO_PLAYLIST_THRESHOLD) {
      const count = AUTO_PLAYLIST_BUFFER - this.playlist.length;
      const excludeIds = this.playlist
        .filter(isTrackItem)
        .map((i) => i.track.id);
      const tracks = await api.generatePlaylist(count, excludeIds);
      this.playlist.push(...tracks.map(trackItem));
      this.updatePrefetch();
      this.scheduleSave();
    }
  }

  /**
   * Push the whole upcoming playlist to the backend prefetch cache: the current
   * track followed by every playlist track in order (stop markers skipped).
   * Called after every playlist mutation and on track changes. The backend byte
   * cap bounds how many leading tracks actually stay resident in RAM.
   */
  private updatePrefetch(): void {
    const upcoming: number[] = this.playlist
      .filter(isTrackItem)
      .map((i) => i.track.id);
    const ids = this.currentTrack
      ? [this.currentTrack.id, ...upcoming]
      : upcoming;
    void api
      .prefetch(ids)
      .catch((err) => logger.error("Prefetch failed:", err));
  }

  setHover(track: Track, rect: DOMRect): void {
    this.hoveredTrack = track;
    this.hoverX = rect.right;
    this.hoverY = rect.top;
  }

  clearHover(): void {
    this.hoveredTrack = null;
  }

  seekToPct(pct: number): void {
    if (!this.duration) return;
    const clamped = Math.min(1, Math.max(0, pct));
    const seconds = clamped * this.duration;
    this.currentTime = seconds;
    void this.backend.seek(seconds).catch((err) => {
      logger.error("Seek failed:", err);
    });
  }

  // ----- Cue deck transport -----

  cueLoadAndPlay(track: Track): void {
    this.cueError = null;
    this.cueTrack = track;
    this.cueDuration = track.duration ?? 0;
    this.cueCurrentTime = 0;
    this.loadCueWaveform(track.id);
    this.loadCueCoverArt(track.id);
    void this.cueBackend
      .load(track.id)
      .then(() => this.cueBackend.play())
      .catch((err) => {
        logger.error("Cue load/play failed:", err);
        this.cueError = err instanceof Error ? err.message : String(err);
      });
  }

  cueTogglePlay(): void {
    if (!this.cueTrack) return;
    if (this.cueIsPlaying) {
      void this.cueBackend.pause();
    } else {
      void this.cueBackend
        .play()
        .catch((err) => logger.error("Cue resume failed:", err));
    }
  }

  cueStop(): void {
    void this.cueBackend.stop();
    this.cueTrack = null;
    this.cueIsPlaying = false;
    this.cueCurrentTime = 0;
    this.cueDuration = 0;
    this.cueWaveform = null;
    this.cueCoverArt = null;
  }

  cueSeekToPct(pct: number): void {
    if (!this.cueDuration) return;
    const clamped = Math.min(1, Math.max(0, pct));
    const seconds = clamped * this.cueDuration;
    this.cueCurrentTime = seconds;
    void this.cueBackend.seek(seconds).catch((err) => {
      logger.error("Cue seek failed:", err);
    });
  }

  setCueVolume(v: number): void {
    this.cueVolume = v;
    void this.cueBackend.setVolume(v);
    this.scheduleSave();
  }

  /**
   * Insert the cue track at the head of the main playlist as next-up.
   * Cue keeps playing — independent transport.
   */
  promoteCueToMain(): void {
    if (!this.cueTrack) return;
    this.playlist.unshift(trackItem(this.cueTrack));
    this.updatePrefetch();
    this.scheduleSave();
  }

  // ----- Audio device config -----

  async loadAudioConfig(): Promise<void> {
    const [devices, main, cue] = await Promise.all([
      api.listAudioDevices(),
      api.getMainDevice(),
      api.getCueDevice(),
    ]);
    this.audioDevices = devices;
    this.mainDevice = main;
    this.cueDevice = cue;
  }

  async setMainDeviceConfig(device: DeviceRef | null): Promise<void> {
    await api.setMainDevice(device);
    this.mainDevice = device;
  }

  async setCueDeviceConfig(device: DeviceRef | null): Promise<void> {
    await api.setCueDevice(device);
    this.cueDevice = device;
    if (device === null) {
      // Cue disabled — clear local cue state.
      this.cueStop();
    }
  }

  async loadStats(): Promise<void> {
    this.stats = await api.getStats();
  }

  async loadSession(): Promise<void> {
    let result: SessionLoadResult;
    try {
      result = await api.loadSession();
    } catch (err) {
      logger.error("Session load failed:", err);
      this.sessionLoaded = true;
      return;
    }
    const { state, tracks } = result;
    const byId = new Map(tracks.map((t) => [t.id, t]));
    const resolve = (ids: number[]): Track[] =>
      ids.map((id) => byId.get(id)).filter((t): t is Track => t !== undefined);

    this.playlist = this.rebuildPlaylist(
      state.playlistItems,
      byId,
      state.playlistIds,
    );
    this.history = resolve(state.historyIds);
    this.autoPlaylistActive = state.autoPlaylistActive;
    this.autoAdvance = state.autoAdvance;
    this.setVolume(state.volume);
    this.setCueVolume(state.cueVolume);

    const restored =
      state.currentTrackId !== null ? byId.get(state.currentTrackId) : null;
    if (restored) {
      this.currentTrack = restored;
      this.duration = restored.duration ?? 0;
      if (state.currentTime > 0) {
        this.currentTime = state.currentTime;
      }
      this.loadWaveform(restored.id);
      this.loadCoverArt(restored.id);
      void this.loadWithSeek(restored, state.currentTime);
      document.title = `${restored.title} - ${restored.artist} | ${APP_NAME}`;
    }

    this.sessionLoaded = true;
    this.updatePrefetch();
  }

  private rebuildPlaylist(
    items: PersistedPlaylistItem[] | undefined,
    byId: Map<number, Track>,
    legacyIds: number[],
  ): PlaylistItem[] {
    if (items && items.length > 0) {
      const out: PlaylistItem[] = [];
      for (const i of items) {
        if (i.kind === "stop") {
          out.push(stopMarker());
        } else {
          const t = byId.get(i.id);
          if (t) out.push(trackItem(t));
        }
      }
      return out;
    }
    return legacyIds
      .map((id) => byId.get(id))
      .filter((t): t is Track => t !== undefined)
      .map(trackItem);
  }

  private async loadWithSeek(track: Track, seek: number): Promise<void> {
    try {
      await this.backend.load(track.id);
      if (seek > 0) {
        await this.backend.seek(seek);
      }
    } catch (err) {
      logger.error("Resume load failed:", err);
    }
  }

  private scheduleSave(): void {
    if (!this.sessionLoaded) return;
    this.throttledSave();
  }

  async flushSave(): Promise<void> {
    this.throttledSave.cancel();
    await this.persistSession();
  }

  private async persistSession(): Promise<void> {
    await api
      .saveSession({
        playlistIds: this.playlist.filter(isTrackItem).map((i) => i.track.id),
        playlistItems: this.playlist.map((i) =>
          isStopMarker(i)
            ? { kind: "stop" as const }
            : { kind: "track" as const, id: i.track.id },
        ),
        historyIds: this.history.map((t) => t.id),
        currentTrackId: this.currentTrack?.id ?? null,
        currentTime: this.currentTime,
        autoPlaylistActive: this.autoPlaylistActive,
        autoAdvance: this.autoAdvance,
        volume: this.volume,
        cueVolume: this.cueVolume,
      })
      .catch((err) => {
        logger.error("Session save failed:", err);
      });
  }

  async loadLibraryPaths(): Promise<void> {
    this.libraryPaths = await api.getAllPaths();
  }

  async addPath(type: ContentType): Promise<void> {
    const added = await api.addPath(type);
    if (added) await this.loadLibraryPaths();
  }

  async removePath(type: ContentType, p: string): Promise<void> {
    await api.removePath(type, p);
    await this.loadLibraryPaths();
  }

  /** Update a track's embedded metadata fields and reflect the change in the local tracks array. */
  async updateTrackMetadata(
    id: number,
    input: Partial<Pick<Track, "title" | "artist" | "album">> & {
      genre?: string | null;
      year?: number | null;
    },
  ): Promise<Track | null> {
    const byIndex = new Map(this.tracks.map((t, i) => [t.id, i]));
    const index = byIndex.get(id);
    let oldTitle = "";
    if (index != null) {
      oldTitle = this.tracks[index].title;
    }
    let updatedTrack: Track;
    try {
      updatedTrack = await api.updateTrackMetadata({
        id,
        title: input.title ?? "",
        artist: input.artist ?? "",
        album: input.album ?? "",
        genre: input.genre ?? null,
        year: input.year ?? null,
      });
    } catch (err) {
      logger.error("updateTrackMetadata failed:", err);
      return null;
    }
    if (index != null) {
      this.tracks[index] = updatedTrack;
    }
    // If the currently playing track was edited, keep its title for document.title.
    if (this.currentTrack?.id === id) {
      this.currentTrack = updatedTrack;
      if (oldTitle && oldTitle !== updatedTrack.title) {
        document.title = `${updatedTrack.title} - ${updatedTrack.artist} | ${APP_NAME}`;
      }
    }
    this.scheduleSave();
    return updatedTrack;
  }

  async scan(): Promise<void> {
    await api.scanLibraries();
  }

  async cancelScan(): Promise<void> {
    await api.cancelScan();
  }

  async hydrateScanStatus(): Promise<void> {
    this.scanStatus = await api.getScanStatus();
  }

  async hydrateWaveformStatus(): Promise<void> {
    this.waveformStatus = await api.getWaveformStatus();
  }

  private async handleEnded(): Promise<void> {
    if (!this.autoAdvance) {
      this.stop();
      return;
    }

    await this.maybeRefillPlaylist();
    this.advancePlan(false);
  }

  /**
   * Decide what to play next given the current cache membership.
   *
   * - `empty`: nothing queued.
   * - `fallback`: no cache knowledge yet (cold start / tests) — use the legacy
   *   "play the head" behavior.
   * - `stop`: a stop marker is the next barrier; honor it.
   * - `play`: the first upcoming cached track (skipping uncached tracks ahead
   *   of it, which stay queued for when the share recovers).
   * - `wait`: cache is known but nothing upcoming is cached — an outage.
   */
  private planAdvance():
    | { kind: "empty" | "fallback" | "wait" }
    | { kind: "stop" | "play"; index: number } {
    if (this.playlist.length === 0) return { kind: "empty" };
    if (this.cachedIds.size === 0) return { kind: "fallback" };
    for (let i = 0; i < this.playlist.length; i++) {
      const item = this.playlist[i];
      if (isStopMarker(item)) return { kind: "stop", index: i };
      if (this.cachedIds.has(item.track.id)) return { kind: "play", index: i };
    }
    return { kind: "wait" };
  }

  /**
   * Advance to the next playable track, skipping uncached ones during an
   * outage. `afterFailure` is set when a load just failed or timed out: in that
   * case an empty cache means "wait for the share" rather than blindly retrying
   * the head (which would burn through the queue on a cold offline start).
   */
  private advancePlan(afterFailure: boolean): void {
    const plan = this.planAdvance();
    switch (plan.kind) {
      case "empty":
        this.clearNetRetry();
        return;
      case "fallback":
        if (afterFailure) {
          this.scheduleNetRetry();
          return;
        }
        this.clearNetRetry();
        this.playIndex(0);
        return;
      case "stop":
      case "play":
        this.clearNetRetry();
        this.playIndex(plan.index);
        return;
      case "wait":
        this.scheduleNetRetry();
        return;
      default:
        return isStrictNever(plan);
    }
  }

  private scheduleNetRetry(): void {
    this.awaitingNetwork = true;
    if (this.netRetryTimer !== null) return;
    const i = Math.min(this.netRetryAttempt, NET_RETRY_BACKOFFS_MS.length - 1);
    this.netRetryAttempt += 1;
    this.netRetryTimer = setTimeout(() => {
      this.netRetryTimer = null;
      // Re-push the prefetch window so the cache worker retries its reads — it
      // only wakes on set_window, so without this an outage never recovers on
      // its own (the share could come back but nothing would re-read it). On a
      // successful read the resulting cache-state advances playback.
      this.updatePrefetch();
      this.advancePlan(true);
    }, NET_RETRY_BACKOFFS_MS[i]);
  }

  private clearNetRetry(): void {
    if (this.netRetryTimer !== null) {
      clearTimeout(this.netRetryTimer);
      this.netRetryTimer = null;
    }
    this.netRetryAttempt = 0;
    this.awaitingNetwork = false;
  }
}

export const app = new AppState();

export function formatTime(seconds: number): string {
  if (!isFinite(seconds)) return "0:00";
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}
