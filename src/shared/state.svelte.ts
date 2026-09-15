import type {
  ContentType,
  DeviceInfo,
  DeviceRef,
  LibraryStats,
  PlaylistItem,
  SortColumn,
  SortDir,
  Track,
  TrackMetadataInput,
  TuningConfig,
} from "./types";
import { isStopMarker, isTrackItem } from "./types";
import {
  api,
  type PlaylistSnapshot,
  type ScanStatus,
  type SessionLoadResult,
  type WaveformStatus,
} from "./api";
import type { DeckBackend, DeckTransport } from "../features/deck/backend";
import { NativeBackend } from "../features/deck/nativeBackend";
import { throttle, type Throttled } from "./throttle";
import { isStrictNever } from "./isStrictNever";
import { APP_NAME } from "./appName";

const logger = {
  error: (...args: unknown[]) => console.error(...args),
};

export type { Track };

// Tuning defaults. Used until `loadTuning()` fetches the persisted config from
// the backend, and as the fallback if that call fails.
const DEFAULT_TUNING: TuningConfig = {
  interleave: {
    jingleEvery: 4,
    commercialEvery: 8,
    commercialBucketMultiplier: 3,
    commercialBucketMin: 10,
  },
  autoPlaylist: {
    // Number of upcoming tracks kept queued by the auto-playlist.
    autoPlaylistBuffer: 20,
    // Refill threshold — below this the auto-playlist tops back up. Kept lower
    // than the buffer to avoid excessive refilling.
    autoPlaylistThreshold: 5,
    historyCap: 100,
    sessionSaveThrottleMs: 500,
    // Backoff schedule (ms) for retrying advancement while nothing playable is
    // cached (network outage). The last value repeats until recovery.
    netRetryBackoffsMs: [1000, 2000, 5000],
  },
  cache: { maxCacheBytes: 150 * 1024 * 1024 },
  player: {
    readWatchdogTimeoutMs: 10000,
    openRetryIntervalMs: 2000,
    readRetryBackoffsMs: [500, 1000, 2000],
  },
};

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

  // Playlist state is owned by the backend and mirrored here from
  // `program:playlist-state` snapshots. Assigning to these fields does not
  // change what goes to air — the `playlist*` commands do.
  playlist = $state<PlaylistItem[]>([]);
  currentTrack = $state<Track | null>(null);
  autoPlaylistActive = $state(false);
  autoAdvance = $state(true);
  // History is the renderer's own: a display log, fed by the `displaced` track
  // in each snapshot.
  history = $state<Track[]>([]);
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
  // True while no upcoming track is cached and the backend is waiting for the
  // share to recover. Mirrored from the snapshot.
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

  // Initialized to defaults; `loadTuning()` replaces it with the persisted
  // config at startup. Read for auto-playlist/history/retry behaviour and
  // edited by the Settings → Advanced tab.
  tuning = $state<TuningConfig>(structuredClone(DEFAULT_TUNING));

  hoveredTrack = $state<Track | null>(null);
  hoverX = $state(0);
  hoverY = $state(0);

  // Track currently being edited via the MetadataEditor overlay.
  editingTrack = $state<Track | null>(null);

  backend: DeckTransport;
  cueBackend: DeckBackend;

  private throttledSave: Throttled;
  private sessionLoaded = false;

  constructor(backend?: DeckTransport, cueBackend?: DeckBackend) {
    this.backend = backend ?? new NativeBackend("main");
    this.cueBackend = cueBackend ?? new NativeBackend("cue");
    this.throttledSave = throttle(
      () => void this.persistSession(),
      this.tuning.autoPlaylist.sessionSaveThrottleMs,
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
          // Advancement is the backend's: it owns the playlist, and it is
          // already listening to this same event.
          break;
        case "buffering":
          this.isBuffering = event.buffering;
          break;
        case "cache-state":
          // A read succeeded, so the share is reachable again.
          this.shareUnreachable = false;
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
          // Read failed after retries or watchdog timeout. The backend skips to
          // the next cached track (or waits for the share) on the same event.
          this.isBuffering = false;
          logger.error("Audio load failed for track:", event.id);
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

    void api.onPlaylistState((snapshot) => this.applySnapshot(snapshot));

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
    this.send(api.playlistAdd(track.id));
  }

  addStopMarker(): void {
    this.send(api.playlistAddStopMarker());
  }

  async addFiller(contentType: ContentType): Promise<void> {
    await api.playlistAddFiller(contentType).catch((err) => {
      logger.error("Add filler failed:", err);
    });
  }

  /**
   * Put a track straight on air, bypassing the playlist. #354 took this off the
   * library row because a button there is too easy to hit by accident during a
   * broadcast; it is reachable from the row's right-click menu (#314), where it
   * sits last and needs a deliberate two-step gesture.
   */
  playNow(track: Track): void {
    this.send(api.playlistPlayNow(track.id));
  }

  removeFromPlaylist(index: number): void {
    this.send(api.playlistRemove(index));
  }

  movePlaylistItem(from: number, to: number): void {
    if (from === to) return;
    this.send(api.playlistMove(from, to));
  }

  clearPlaylist(): void {
    this.send(api.playlistClear());
  }

  playIndex(index: number): void {
    if (index < 0 || index >= this.playlist.length) return;
    this.send(api.playlistPlayIndex(index));
  }

  /** Fire a playlist command; the snapshot it produces is what updates the UI. */
  private send(call: Promise<void>): void {
    void call.catch((err) => logger.error("Playlist command failed:", err));
  }

  get historyDisplay(): Track[] {
    return this.history.slice().reverse();
  }

  appendHistory(track: Track): void {
    const cap = this.tuning.autoPlaylist.historyCap;
    this.history.push(track);
    if (this.history.length > cap) {
      this.history.splice(0, this.history.length - cap);
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
    this.send(api.playlistAdd(track.id));
  }

  /**
   * Adopt a backend snapshot. This is the only writer of playlist state: the
   * queue, what is on air, and the auto flags are all projections of it.
   */
  private applySnapshot(snapshot: PlaylistSnapshot): void {
    this.playlist = snapshot.playlist;
    this.autoPlaylistActive = snapshot.autoPlaylistActive;
    this.autoAdvance = snapshot.autoAdvance;
    this.awaitingNetwork = snapshot.awaitingNetwork;
    if (snapshot.displaced) this.appendHistory(snapshot.displaced);
    const changed =
      (this.currentTrack?.id ?? null) !== (snapshot.current?.id ?? null);
    this.currentTrack = snapshot.current;
    if (changed) this.onCurrentChanged(snapshot.current);
    this.scheduleSave();
  }

  /**
   * Redraw everything keyed to the track on air. Guarded on the id actually
   * changing: snapshots arrive on every playlist mutation, and refetching the
   * waveform and artwork of an unchanged track would flash the deck.
   */
  private onCurrentChanged(track: Track | null): void {
    this.currentTime = 0;
    if (!track) {
      this.duration = 0;
      this.waveform = null;
      this.coverArt = null;
      this.isPlaying = false;
      document.title = APP_NAME;
      return;
    }
    // Optimistic until the deck reports the decoded duration, which is the one
    // that counts on a VBR file with a wrong tag.
    this.duration = track.duration ?? 0;
    this.loadWaveform(track.id);
    this.loadCoverArt(track.id);
    document.title = `${track.title} - ${track.artist} | ${APP_NAME}`;
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
    this.send(api.playlistStop());
  }

  next(): void {
    this.send(api.playlistNext());
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
    this.send(api.playlistPrev(previous.id));
  }

  toggleMode(): void {
    this.send(api.playlistSetAutoAdvance(!this.autoAdvance));
  }

  async toggleAutoPlaylist(): Promise<void> {
    await api
      .playlistSetAutoPlaylist(!this.autoPlaylistActive)
      .catch((err) => logger.error("Auto-playlist toggle failed:", err));
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
    this.send(api.playlistAddFront(this.cueTrack.id));
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

  /// Fetch persisted tuning from the backend. Falls back to defaults (already in
  /// place) on error so a backend hiccup never leaves the app unusable.
  async loadTuning(): Promise<void> {
    try {
      this.applyTuning(await api.getTuningConfig());
    } catch (err) {
      logger.error("Failed to load tuning config", err);
    }
  }

  /// Persist edited tuning and adopt the backend's clamped result. Cache/player
  /// fields only take effect on restart (their worker threads capture them at
  /// startup); the renderer-side fields applied here take effect immediately.
  async saveTuning(next: TuningConfig): Promise<void> {
    this.applyTuning(await api.setTuningConfig(next));
  }

  /// Adopt a tuning config: store it and rebuild the session-save throttle,
  /// since its interval is derived from `sessionSaveThrottleMs`.
  private applyTuning(tuning: TuningConfig): void {
    this.tuning = tuning;
    this.throttledSave.cancel();
    this.throttledSave = throttle(
      () => void this.persistSession(),
      tuning.autoPlaylist.sessionSaveThrottleMs,
    );
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
    this.history = state.historyIds
      .map((id) => byId.get(id))
      .filter((t): t is Track => t !== undefined);
    // Master level is fixed at unity (#354): the volume slider left the operator
    // UI, so a persisted value from an older session would be unrecoverable.
    // Normalization is ReplayGain's job (#80), not the operator's.
    this.setVolume(1);
    this.setCueVolume(state.cueVolume);

    // The backend restored the playlist from the same session file and put the
    // saved track back on the deck. Pull its state rather than waiting for a
    // snapshot that was emitted before this window was listening.
    try {
      this.applySnapshot(await api.playlistSync());
    } catch (err) {
      logger.error("Playlist sync failed:", err);
    }
    // Same reason, for the deck: whatever `pause-state` the deck emitted while
    // this window was still starting up went nowhere, so ask it directly. Left
    // at its default if the call fails — a wrong transport button is better
    // than no session.
    try {
      this.isPlaying = await api.mainDeckIsPlaying();
    } catch (err) {
      logger.error("Deck state sync failed:", err);
    }
    // After the snapshot: adopting one resets the clock, and the saved position
    // is what the deck is actually parked at.
    if (this.currentTrack && state.currentTime > 0) {
      this.currentTime = state.currentTime;
    }

    this.sessionLoaded = true;
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
        volume: 1,
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
    // Forward only the fields the caller actually set (partial patch); an
    // absent key leaves that column unchanged on the backend.
    const payload: TrackMetadataInput = { id };
    if (input.title !== undefined) payload.title = input.title;
    if (input.artist !== undefined) payload.artist = input.artist;
    if (input.album !== undefined) payload.album = input.album;
    if (input.genre !== undefined) payload.genre = input.genre;
    if (input.year !== undefined) payload.year = input.year;
    let updatedTrack: Track;
    try {
      updatedTrack = await api.updateTrackMetadata(payload);
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
}

export const app = new AppState();

export function formatTime(seconds: number): string {
  if (!isFinite(seconds)) return "0:00";
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}
