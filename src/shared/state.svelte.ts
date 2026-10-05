import type {
  AdminStatus,
  Appearance,
  ImageSlot,
  ThemeListing,
  ContentType,
  LibraryTab,
  SavedEntry,
  SavedPlaylist,
  SavedPlaylistSummary,
  SourceInfo,
  CuePoints,
  DeviceInfo,
  DeviceRef,
  HealthReport,
  FindingKind,
  LibraryStats,
  PlaylistItem,
  Recalculated,
  SortColumn,
  SortDir,
  Track,
  TrackMetadataInput,
  TuningConfig,
  UpdateState,
} from "./types";
import { METADATA_KEYS } from "./types";
import { isStopMarker, isTrackItem } from "./types";
import {
  airDuration,
  airedTrack,
  cuePointsEqual,
  NO_CUE_POINTS,
  queueAirTime,
  resolveCuePoints,
} from "./cuePoints";
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
import {
  allSelected,
  selectAll,
  selectRange,
  toggle,
  without,
} from "./selection";
import { isStrictNever } from "./isStrictNever";
import { APP_NAME } from "./appName";
import { savePaintHint } from "./appearance";
import { healthAttention as attentionOf } from "./health";
import { appendMessage, importMessage, sizeLabel } from "./savedPlaylists";

const logger = {
  error: (...args: unknown[]) => console.error(...args),
};

export type { Track };

/**
 * Tuning defaults. Used until `loadTuning()` fetches the persisted config from
 * the backend, and as the fallback if that call fails.
 */
const DEFAULT_TUNING: TuningConfig = {
  interleave: {
    jingleEvery: 4,
    commercialEvery: 8,
    commercialBucketMultiplier: 3,
    commercialBucketMin: 10,
  },
  autoPlaylist: {
    autoPlaylistBuffer: 20,
    autoPlaylistThreshold: 5,
    historyCap: 100,
    sessionSaveThrottleMs: 500,
    netRetryBackoffsMs: [1000, 2000, 5000],
  },
  rotation: { titleWindowMin: 180, artistWindowMin: 45 },
  cache: { maxCacheBytes: 150 * 1024 * 1024 },
  player: {
    readWatchdogTimeoutMs: 10000,
    deadAirLimitMs: 3000,
    openRetryIntervalMs: 2000,
    readRetryBackoffsMs: [500, 1000, 2000],
    fadeOutMs: 4000,
    fadeToNextMs: 2500,
    replayGain: "track",
  },
  library: {
    checkIntervalMin: 15,
    scanOnChanges: false,
    writeTags: false,
    tagWriteTimeoutSec: 30,
  },
  autoCue: {
    apply: true,
    applyNextStart: true,
    silenceDbfs: -70,
    segueDbfs: -20,
  },
  updates: { autoCheck: true },
};

export const EMPTY_HEALTH: HealthReport = {
  missing: [],
  missingDismissed: false,
  exact: [],
  possible: [],
  unhashed: 0,
  unreadable: [],
  badDurations: [],
  badDurationsDismissed: false,
  check: null,
  checkDismissed: false,
  checking: false,
  tagWriteFailures: [],
};

export type PlaylistTab = "playlist" | "history";
export type SettingsTab =
  | "audio"
  | "library"
  | "playlist"
  | "now-playing"
  | "appearance"
  | "advanced"
  | "about";

/**
 * What the cue deck was showing when a surface borrowed it. `previewing` rather
 * than the marker set itself: the deck's *Preview* means "the track's own
 * markers", and those may have been saved over in between.
 */
export interface CueSnapshot {
  track: Track | null;
  previewing: boolean;
}

const messageOf = (err: unknown): string =>
  err instanceof Error ? err.message : String(err);

/** Which saved-playlist dialog is up, and what it acts on. */
export type SavedDialog =
  | { kind: "saveAs" }
  /** `fromSelection` clears the library selection once the tracks are in. */
  | { kind: "addTo"; trackIds: number[]; fromSelection?: boolean }
  | { kind: "rename"; id: number; name: string }
  | { kind: "delete"; id: number; name: string };

export class AppState {
  searchQuery = $state("");
  activeTab = $state<LibraryTab>("music");

  /** The saved playlists, as the backend last listed them. */
  savedPlaylists = $state<SavedPlaylistSummary[]>([]);
  /** The saved playlist open in the Playlists tab. See `docs/saved-playlists.md`. */
  openSaved = $state<SavedPlaylist | null>(null);
  private openSavedRequest = 0;
  /** What the last append of a saved playlist did. */
  savedNotice = $state<string | null>(null);
  /** The saved-playlist dialog that is up, if one is. */
  savedDialog = $state<SavedDialog | null>(null);
  /** The entry _Find in library_ is up for, if it is. */
  findingFor = $state<SavedEntry | null>(null);
  playlistTab = $state<PlaylistTab>("playlist");

  /**
   * The library rows being dragged, for whichever list they are dropped on:
   * one row, or the whole selection when the row under the pointer is in it.
   */
  draggedTrackIds = $state<number[] | null>(null);
  /** What is being dragged is the library selection, which a drop then uses up. */
  private draggedLibrarySelection = false;

  /**
   * The library selection, as track ids in pick order. It outlives a search, a
   * sort and a tab change, so it may name tracks that are not in
   * {@link AppState.tracks}. See `docs/library.md`.
   */
  selectedIds = $state<number[]>([]);
  /** The row a range is measured from: the last one picked or dropped. */
  private selectionAnchor: number | null = null;
  sortBy = $state<SortColumn | null>(null);
  sortDir = $state<SortDir>("asc");
  tracks = $state<Track[]>([]);

  /**
   * Bumped per {@link AppState.search} call, so a result that is no longer the
   * newest one asked for is dropped rather than drawn. Deliberately not
   * `$state`: nothing renders it, and a reactive write per keystroke would only
   * invalidate what reads it.
   */
  private searchRequest = 0;
  stats = $state<LibraryStats | null>(null);
  libraryPaths = $state<Record<ContentType, string[]>>({
    music: [],
    commercial: [],
    jingle: [],
  });

  /**
   * Whether `libraryPaths` has been read from the backend yet. The empty
   * object above is indistinguishable from a station with no directories
   * configured, and "no directories defined" is the wrong thing to tell an
   * operator who has three.
   */
  libraryPathsLoaded = $state(false);
  settingsOpen = $state(false);
  settingsTab = $state<SettingsTab>("library");
  scanStatus = $state<ScanStatus>({ status: "idle", lastResult: null });
  /** Progress of the background waveform pass, which runs after the scan. */
  waveformStatus = $state<WaveformStatus>({ status: "idle" });

  /**
   * The queue, mirrored from `program:playlist-state` snapshots. The backend
   * owns it: assigning here does not change what goes to air, the `playlist*`
   * commands do. The same holds for every mirrored field below.
   */
  playlist = $state<PlaylistItem[]>([]);
  upcomingAirTime = $derived(queueAirTime(this.playlist));

  /**
   * The override the track on air is playing under, when it came off an item
   * that carried one.
   */
  currentCueOverride = $state<CuePoints | null>(null);
  currentTrack = $state<Track | null>(null);

  /**
   * Which live fade is running, for the button that started it. The ramp itself
   * is the backend's; this only drives the progress the operator sees, animated
   * locally over `fadeMs` rather than stepped over IPC.
   */
  fading = $state<"out" | "next" | null>(null);
  fadeMs = $state(0);
  autoPlaylistActive = $state(false);
  /** The saved playlist the auto-playlist draws from; `null` is the music library. */
  autoSource = $state<SourceInfo | null>(null);
  /** A source the backend dropped for want of playable music. */
  revertedFrom = $state<string | null>(null);

  /**
   * The source was changed over a playlist that already held tracks, which air
   * before anything the new source picks. Said once, until the queue is empty
   * or the operator dismisses it.
   */
  sourceSwitchQueued = $state(false);
  autoAdvance = $state(true);
  /** What has aired, oldest first: the airing log's tail, from the snapshot. */
  history = $state<Track[]>([]);

  /**
   * The outgoing track of a handover while it is still audible on the program
   * bus: the tail deck's track and how much of it is left. `null` whenever no
   * overlap is in progress, which is most of the time.
   */
  tailTrackId = $state<number | null>(null);
  tailDuration = $state(0);
  tailRemaining = $state(0);
  isPlaying = $state(false);
  isBuffering = $state(false);
  volume = $state(1);
  currentTime = $state(0);
  duration = $state(0);

  /**
   * Amplitude-curve peaks (0..=255 per bucket) for the current main-deck track,
   * rendered behind the seek bar. `null` while none is loaded or the track has
   * no stored waveform, which falls back to a plain progress bar.
   */
  waveform = $state<number[] | null>(null);

  /**
   * Base64 `data:` URL of the current main-deck track's embedded cover art,
   * shown on the spinning vinyl disc. `null` while none is loaded or the track
   * has no artwork, which falls back to a note-icon placeholder.
   */
  coverArt = $state<string | null>(null);

  /**
   * True while no upcoming track is cached and the backend is waiting for the
   * share to recover.
   */
  awaitingNetwork = $state(false);

  /**
   * True while a prefetch read is failing — the share looks unreachable even if
   * the current in-RAM track keeps playing. Set by `prefetch-failed`, cleared
   * by the next `cache-state`, which means a read succeeded.
   */
  shareUnreachable = $state(false);

  /**
   * True while the main deck cannot open an audio output device: none present,
   * or the configured one won't open. The backend auto-retries and clears it on
   * recovery. Distinct from a network outage — the device, not the share.
   */
  outputUnavailable = $state(false);

  /**
   * True from a launch that replaced an older library database until the scan
   * that repopulates it finishes.
   */
  libraryReset = $state(false);
  /** Missing tracks, duplicates and disk changes, from `library-health`. */
  health = $state<HealthReport>(structuredClone(EMPTY_HEALTH));

  /**
   * When each missing track went missing, keyed by id — what the playlist,
   * history and deck badge their rows from.
   */
  missingSince = $derived(
    new Map(this.health.missing.map((t) => [t.id, t.missingSince])),
  );
  /** Findings that want attention; badges the Settings button. */
  healthAttention = $derived(attentionOf(this.health));

  /** The updater, mirrored whole from `update:state`. */
  update = $state<UpdateState>({
    currentVersion: "",
    phase: { kind: "idle" },
    offer: null,
  });
  /** Whether a newer release is waiting, for the Settings badge. */
  updateWaiting = $derived(this.update.offer !== null);

  // ----- Cue deck (independent transport on a separate audio device) -----
  cueTrack = $state<Track | null>(null);
  cueIsPlaying = $state(false);
  cueIsBuffering = $state(false);
  cueCurrentTime = $state(0);
  cueDuration = $state(0);
  cueVolume = $state(1);
  cueError = $state<string | null>(null);
  /** {@link AppState.outputUnavailable}, for the cue deck. */
  cueOutputUnavailable = $state(false);
  /** {@link AppState.waveform}, for the cue deck. */
  cueWaveform = $state<number[] | null>(null);
  /** {@link AppState.coverArt}, for the cue deck. */
  cueCoverArt = $state<string | null>(null);

  /**
   * Cue points the cue deck was last loaded with, or `null` for an *Absolute*
   * audition of the whole file. Drives `cueMode` and the cropped waveform.
   */
  cueAppliedPoints = $state<CuePoints | null>(null);

  // ----- Audio device config -----
  audioDevices = $state<DeviceInfo[]>([]);
  mainDevice = $state<DeviceRef | null>(null);
  cueDevice = $state<DeviceRef | null>(null);

  /**
   * Initialized to defaults; `loadTuning()` replaces it with the persisted
   * config at startup. Read for auto-playlist, history and retry behaviour,
   * and edited by the Settings → Advanced tab.
   */
  tuning = $state<TuningConfig>(structuredClone(DEFAULT_TUNING));

  /**
   * The resolved appearance. Replaced by `loadAppearance()` before the app
   * mounts; until then the static `:root` palette in `styles.css` paints.
   */
  appearance = $state<Appearance | null>(null);

  /** Token names currently set on &lt;html>, so a theme switch can clear them. */
  #appliedTokens: string[] = [];

  /**
   * What the operator calls their station, falling back to the product name.
   * A theme never sets this — it is station identity, not palette.
   */
  get brandName(): string {
    return this.appearance?.stationName ?? APP_NAME;
  }

  /** Whether a playing deck turns its vinyl art. On until the operator says otherwise. */
  get spinVinyl(): boolean {
    return this.appearance?.spinVinyl ?? true;
  }

  /** Every theme the operator can pick, invalid ones included. */
  themes = $state<ThemeListing[]>([]);

  hoveredTrack = $state<Track | null>(null);
  hoverX = $state(0);
  hoverY = $state(0);

  /** Track whose tags are open in the metadata overlay. */
  editingMetadata = $state<Track | null>(null);
  /** Track whose cue points are open in the cue-point editor. */
  editingCuePoints = $state<Track | null>(null);

  /**
   * Set by the cue editor: its draft differs from the stored markers. Read when
   * an automatic result lands for the track being edited — see
   * {@link AppState.adoptCuePoints}.
   */
  cueEditorDirty = $state(false);

  /**
   * Admin mode, mirrored from the backend. With no password set the app is
   * always admin. See `docs/admin-mode.md`.
   */
  admin = $state<AdminStatus>({
    passwordSet: false,
    unlocked: true,
    idleLockMin: 15,
  });
  isAdmin = $derived(!this.admin.passwordSet || this.admin.unlocked);
  unlockOpen = $state(false);

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
          // A fade that reached silence stopped the deck; so did anything else
          // that paused it. Either way there is no ramp left to show.
          if (event.paused) this.fading = null;
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
          this.shareUnreachable = false;
          break;
        case "prefetch-failed":
          this.shareUnreachable = true;
          break;
        case "output-unavailable":
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

    // The outgoing track of a handover, while it is still audible. Without
    // this the operator hears two tracks with nothing on screen saying so.
    void api.onDeckRoles((roles) => {
      const tail = roles.find((r) => r.role === "tail");
      this.tailTrackId = tail?.trackId ?? null;
      if (this.tailTrackId === null) this.tailRemaining = 0;
    });
    void api.onTailDuration((seconds) => (this.tailDuration = seconds));
    void api.onTailTime(
      (seconds) =>
        (this.tailRemaining = Math.max(0, this.tailDuration - seconds)),
    );
    void api.onLibraryHealth((report) => (this.health = report));
    void api.onSavedPlaylists((list) => void this.applySavedPlaylists(list));
    void api.onAdminStateChanged((status) => this.applyAdmin(status));
    void api.onUpdateState((state) => (this.update = state));

    api.onScanProgress(({ processed, total }) => {
      if (this.scanStatus.status === "running") {
        this.scanStatus = { status: "running", processed, total };
      }
    });

    api.onScanStateChanged((next) => {
      const wasRunning = this.scanStatus.status === "running";
      this.scanStatus = next;
      if (wasRunning && next.status !== "running") {
        this.libraryReset = false;
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
    api.onCuePointsReady((id, points) => this.adoptCuePoints(id, points));
    api.onDurationReady((id, duration) => this.adoptDuration(id, duration));

    api.onWaveformProgress(({ processed, total }) => {
      if (this.waveformStatus.status === "running") {
        this.waveformStatus = { status: "running", processed, total };
      }
    });
    api.onWaveformStateChanged((next) => {
      this.waveformStatus = next;
    });
    // The backfill rewrites tag columns on rows the panel is already showing,
    // and `tracks` is a snapshot from the last query. One re-read when the pass
    // finishes, rather than one per track it touches.
    api.onTagBackfillStateChanged((next) => {
      if (next.status !== "idle") return;
      void this.search().then(() => {
        // An editor opened before the pass ran holds a snapshot with the tag
        // columns still empty. Saving it would send those blanks back as
        // deliberate clears — flagged as operator edits, and so written to the
        // file. Re-point it at the refreshed row.
        const open = this.editingMetadata;
        if (!open) return;
        const fresh = this.tracks.find((t) => t.id === open.id);
        if (fresh) this.editingMetadata = fresh;
      });
    });
  }

  get progressPct(): number {
    return this.duration ? (this.currentTime / this.duration) * 100 : 0;
  }

  /**
   * Seconds until the station goes quiet if nobody touches it: the rest of the
   * track on air, plus the queue up to its first stop marker when Auto is
   * advancing it. `null` with nothing on air.
   */
  get airTimeRemaining(): number | null {
    if (!this.currentTrack) return null;
    const rest = Math.max(0, this.duration - this.currentTime);
    return this.autoAdvance ? rest + this.upcomingAirTime : rest;
  }

  /**
   * The outgoing track still audible under the one on air, or `null` when no
   * handover is overlapping. Resolved out of history — where the handover put
   * the outgoing track — since `program:roles` carries ids.
   */
  get tailTrack(): Track | null {
    if (this.tailTrackId === null) return null;
    for (let i = this.history.length - 1; i >= 0; i--) {
      if (this.history[i].id === this.tailTrackId) return this.history[i];
    }
    return null;
  }

  /**
   * Drives the "Reconnecting…" banner: playback is blocked waiting for the
   * share, or a prefetch read is currently failing.
   */
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

  /**
   * Re-query the library. Keeps the current rows on error rather than
   * rejecting into a void call, so a query the backend refuses leaves the
   * listing usable instead of silently frozen.
   *
   * Two queries can be in flight at once — the backend answers them on its
   * blocking pool, not in the order they were asked — so a result is adopted
   * only while it is still the newest one asked for. Without that, a slow query
   * landing after a faster later one would put stale rows on screen and leave
   * them there.
   */
  async search(): Promise<void> {
    const tab = this.activeTab;
    if (tab === "playlists") return;
    const request = ++this.searchRequest;
    try {
      const tracks = await api.search(
        this.searchQuery,
        tab,
        this.sortBy ?? undefined,
        this.sortDir,
      );
      if (request === this.searchRequest) this.tracks = tracks;
    } catch (err) {
      logger.error("Search failed:", err);
    }
  }

  setTab(tab: LibraryTab): void {
    this.activeTab = tab;
    void this.search();
  }

  // ----- Saved playlists -----

  async loadSavedPlaylists(): Promise<void> {
    try {
      this.savedPlaylists = await api.savedPlaylistList();
    } catch (err) {
      logger.error("Saved playlists lookup failed:", err);
    }
  }

  /** A new list also refreshes the open one: its entries may be what changed. */
  private async applySavedPlaylists(
    list: SavedPlaylistSummary[],
  ): Promise<void> {
    this.savedPlaylists = list;
    if (this.openSaved) await this.openSavedPlaylist(this.openSaved.id);
  }

  /** Open a saved playlist, or close it when it is no longer there. */
  async openSavedPlaylist(id: number): Promise<void> {
    const request = ++this.openSavedRequest;
    try {
      const saved = await api.savedPlaylistGet(id);
      if (request === this.openSavedRequest) this.openSaved = saved;
    } catch (err) {
      logger.error("Saved playlist lookup failed:", err);
    }
  }

  closeSavedPlaylist(): void {
    this.openSavedRequest++;
    this.openSaved = null;
  }

  /** Append a saved playlist to the playlist and say what that did. */
  async addSavedToPlaylist(id: number, weave: boolean): Promise<void> {
    try {
      const { added, skipped } = await api.playlistAddSaved(id, weave);
      this.savedNotice = appendMessage(added, skipped);
    } catch (err) {
      logger.error("Adding a saved playlist failed:", err);
    }
  }

  /**
   * Choose the auto-playlist source: a saved playlist, or `null` for the music
   * library. Sets the source and nothing else; the snapshot that follows is
   * what shows it. A refusal is said in the notice.
   */
  async setAutoSource(id: number | null): Promise<void> {
    try {
      await api.playlistSetSource(id);
      this.sourceSwitchQueued = this.playlist.length > 0;
    } catch (err) {
      this.savedNotice = messageOf(err);
    }
  }

  /** Pick a file and make a saved playlist of it. The notice says how it went. */
  async importSavedPlaylist(): Promise<void> {
    try {
      const path = await api.pickSavedPlaylistFile();
      if (path === null) return;
      const made = await api.savedPlaylistImport(path);
      this.savedNotice = importMessage(made.name, made.entries, made.missing);
    } catch (err) {
      this.savedNotice = `Import failed: ${messageOf(err)}`;
    }
  }

  /** Ask where, and write the saved playlist there as a file. */
  async exportSavedPlaylist(id: number, name: string): Promise<void> {
    try {
      const path = await api.pickSavedPlaylistTarget(name);
      if (path === null) return;
      await api.savedPlaylistExport(id, path);
      this.savedNotice = `Exported “${name}”`;
    } catch (err) {
      this.savedNotice = `Export failed: ${messageOf(err)}`;
    }
  }

  /** Rejects with the backend's reason, for the dialog to show. */
  async savePlaylistAs(name: string): Promise<void> {
    await api.playlistSaveAs(name);
  }

  /** Rejects with the backend's reason, for the dialog to show. */
  async createSavedPlaylist(name: string, trackIds: number[]): Promise<void> {
    const made = await api.savedPlaylistCreate(name, trackIds);
    this.savedNotice = `Saved “${made.name}”: ${sizeLabel(made.entries, 0)}`;
  }

  /** Rejects with the backend's reason, for the dialog to show. */
  async addToSavedPlaylist(id: number, trackIds: number[]): Promise<void> {
    await api.savedPlaylistAddEntries(id, trackIds, null);
  }

  /**
   * Offer the library selection to a saved playlist, in pick order: a new one,
   * or for an admin an existing one. The selection is cleared once the dialog
   * has put the tracks somewhere, not before.
   */
  saveSelection(): void {
    if (this.selectedIds.length === 0) return;
    this.savedDialog = {
      kind: "addTo",
      trackIds: [...this.selectedIds],
      fromSelection: true,
    };
  }

  /** Rejects with the backend's reason, for the dialog to show. */
  async renameSavedPlaylist(id: number, name: string): Promise<void> {
    await api.savedPlaylistRename(id, name);
  }

  /** Rejects with the backend's reason, for the dialog to show. */
  async deleteSavedPlaylist(id: number): Promise<void> {
    await api.savedPlaylistDelete(id);
    if (this.openSaved?.id === id) this.closeSavedPlaylist();
  }

  /** Queue tracks in the order given, at the end or as next-up. */
  queueTracks(ids: number[], asNext = false): void {
    if (ids.length === 0) return;
    this.send(api.playlistAddMany(ids, asNext ? 0 : null));
  }

  removeSavedEntries(entryIds: number[]): void {
    const saved = this.openSaved;
    if (!saved || entryIds.length === 0) return;
    this.send(api.savedPlaylistRemoveEntries(saved.id, entryIds));
  }

  /** Move entries of the open saved playlist as one block into the gap at `index`. */
  moveSavedEntries(entryIds: number[], index: number): void {
    const saved = this.openSaved;
    if (!saved || entryIds.length === 0) return;
    this.send(api.savedPlaylistMoveEntries(saved.id, entryIds, index));
  }

  /**
   * The tracks of one type a search finds, for _Find in library_. It leaves
   * the library's own search and rows alone.
   */
  findTracks(query: string, contentType: ContentType): Promise<Track[]> {
    return api.search(query, contentType);
  }

  /** Rejects with the backend's reason, for the dialog to show. */
  async bindSavedEntry(entryId: number, trackId: number): Promise<void> {
    await api.savedPlaylistBindEntry(entryId, trackId);
  }

  removeSavedEntry(entryId: number): void {
    this.send(api.savedPlaylistRemoveEntry(entryId));
  }

  moveSavedEntry(from: number, to: number): void {
    const saved = this.openSaved;
    if (!saved) return;
    this.send(api.savedPlaylistMoveEntry(saved.id, from, to));
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

  /** Queue a track as next-up, ahead of everything already queued. */
  addNextToPlaylist(track: Track): void {
    this.send(api.playlistAddFront(track.id));
  }

  /** The rows a selection gesture can reach: none on the Playlists tab. */
  get listedIds(): number[] {
    if (this.activeTab === "playlists") return [];
    return this.tracks.map((track) => track.id);
  }

  toggleSelected(id: number): void {
    this.selectedIds = toggle(this.selectedIds, id);
    this.selectionAnchor = id;
  }

  /** Pick every row from the last one picked to this one. */
  selectRangeTo(id: number): void {
    this.selectedIds = selectRange(
      this.selectedIds,
      this.listedIds,
      this.selectionAnchor,
      id,
    );
    this.selectionAnchor = id;
  }

  /** Pick every listed row, or drop them all once every one is picked. */
  toggleSelectAll(): void {
    const listed = this.listedIds;
    this.selectedIds = allSelected(this.selectedIds, listed)
      ? without(this.selectedIds, listed)
      : selectAll(this.selectedIds, listed);
  }

  clearSelection(): void {
    this.selectedIds = [];
    this.selectionAnchor = null;
  }

  /** Queue the selection in pick order, at the end or as next-up, and clear it. */
  addSelectionToPlaylist(asNext = false): void {
    if (this.selectedIds.length === 0) return;
    this.send(api.playlistAddMany([...this.selectedIds], asNext ? 0 : null));
    this.clearSelection();
  }

  /** A drag of a selected row carries the selection; of any other, that row. */
  startLibraryDrag(track: Track): void {
    this.draggedLibrarySelection = this.selectedIds.includes(track.id);
    this.draggedTrackIds = this.draggedLibrarySelection
      ? [...this.selectedIds]
      : [track.id];
  }

  /**
   * Start a drag of tracks that are not the library selection's — the entries
   * of a saved playlist. An empty list is a drag that carries no track.
   */
  startTrackDrag(ids: number[]): void {
    this.draggedLibrarySelection = false;
    this.draggedTrackIds = ids.length > 0 ? ids : null;
  }

  /** Queue what is being dragged ahead of the item at `index`. */
  dropDraggedInPlaylist(index: number): void {
    const ids = this.draggedTrackIds;
    this.draggedTrackIds = null;
    if (!ids || ids.length === 0) return;
    const selection = this.draggedLibrarySelection;
    if (ids.length === 1) this.send(api.playlistInsert(ids[0], index));
    else this.send(api.playlistAddMany(ids, index));
    if (selection) this.clearSelection();
  }

  revealTrack(track: Track): void {
    void api.revealTrack(track.id).catch((err) => {
      logger.error("Reveal track failed:", err);
    });
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

  /**
   * Adopt a backend snapshot. This is the only writer of playlist state: the
   * queue, what is on air, and the auto flags are all projections of it.
   */
  private applySnapshot(snapshot: PlaylistSnapshot): void {
    this.playlist = snapshot.playlist;
    this.currentCueOverride = snapshot.currentOverride ?? null;
    this.autoPlaylistActive = snapshot.autoPlaylistActive;
    this.autoSource = snapshot.source ?? null;
    this.revertedFrom = snapshot.revertedFrom ?? null;
    if (snapshot.playlist.length === 0) this.sourceSwitchQueued = false;
    this.autoAdvance = snapshot.autoAdvance;
    this.awaitingNetwork = snapshot.awaitingNetwork;
    this.history = snapshot.history;
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
    // A fade to next is over the moment the incoming track is on air: the ramp
    // that is still running belongs to the tail, not to the transport.
    this.fading = null;
    this.currentTime = 0;
    if (!track) {
      this.duration = 0;
      this.waveform = null;
      this.coverArt = null;
      this.isPlaying = false;
      document.title = this.brandName;
      return;
    }
    // Optimistic until the deck reports the decoded duration, which is the one
    // that counts on a VBR file with a wrong tag. Air time, not file time: the
    // deck reports air time too, so the two never disagree about what a
    // trimmed track's bar means.
    this.duration = airDuration(airedTrack(track, this.currentCueOverride));
    this.loadWaveform(track.id);
    this.loadCoverArt(track.id);
    document.title = `${track.title} - ${track.artist} | ${this.brandName}`;
  }

  /**
   * Fetch something that belongs to the track on `deck` and store it, guarding
   * against a race: a slower fetch for a track the operator has already
   * skipped past must not overwrite what is on screen now. The field is
   * cleared on the way in, and the result is dropped unless `id` is still the
   * track on that deck when it arrives.
   */
  private loadForDeck<T>(
    deck: "main" | "cue",
    id: number,
    fetch: (id: number) => Promise<T>,
    store: (value: T | null) => void,
    what: string,
  ): void {
    store(null);
    void fetch(id)
      .then((value) => {
        const on = deck === "main" ? this.currentTrack : this.cueTrack;
        if (on?.id === id) store(value);
      })
      .catch((err) => logger.error(`${what} load failed:`, err));
  }

  private loadWaveform(id: number): void {
    this.loadForDeck(
      "main",
      id,
      (i) => api.getWaveform(i),
      (v) => (this.waveform = v),
      "Waveform",
    );
  }

  private loadCueWaveform(id: number): void {
    this.loadForDeck(
      "cue",
      id,
      (i) => api.getWaveform(i),
      (v) => (this.cueWaveform = v),
      "Cue waveform",
    );
  }

  private loadCoverArt(id: number): void {
    this.loadForDeck(
      "main",
      id,
      (i) => api.getCoverArt(i),
      (v) => (this.coverArt = v),
      "Cover art",
    );
  }

  private loadCueCoverArt(id: number): void {
    this.loadForDeck(
      "cue",
      id,
      (i) => api.getCoverArt(i),
      (v) => (this.cueCoverArt = v),
      "Cue cover art",
    );
  }

  togglePlay(): void {
    // Play is how a fade is aborted: the backend cancels the ramp, so the
    // button must stop showing one.
    this.fading = null;
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
    this.fading = null;
    this.send(api.playlistStop());
  }

  next(): void {
    this.fading = null;
    this.send(api.playlistNext());
  }

  /**
   * Ramp the on-air deck to silence, then stop. Pressing again while the ramp
   * runs finishes it immediately — mid-fade, the operator wants it gone, not
   * restarted.
   */
  fadeOut(): void {
    const finishing = this.fading === "out";
    this.send(api.mainDeckFadeOut(finishing ? 0 : undefined));
    if (finishing) {
      this.fading = null;
      return;
    }
    this.fading = "out";
    this.fadeMs = this.tuning.player.fadeOutMs;
  }

  /** Start the next item now and fade this one out underneath it. */
  fadeToNext(): void {
    const finishing = this.fading === "next";
    this.send(api.mainDeckFadeToNext(finishing ? 0 : undefined));
    if (finishing) {
      this.fading = null;
      return;
    }
    this.fading = "next";
    this.fadeMs = this.tuning.player.fadeToNextMs;
  }

  prev(): void {
    this.fading = null;
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
    this.send(api.playlistPrev());
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

  /**
   * Put a track on the cue deck, **parked**. Cueing is a staging action: the
   * operator decides when it makes noise. Loading on play would also restart
   * the audio on every Absolute/Preview toggle, since switching mode reloads
   * the deck.
   *
   * `cuePoints` picks the mode: `null` is *Absolute* — the whole file, nothing
   * applied, which is what an operator scrubs to find an in-point. Anything
   * else is *Preview*, including an unsaved draft from the cue editor, so a
   * ramp can be heard before it is committed.
   *
   * `autoplay` is the one exception to parking, and travels with the load
   * because the deck parks the sink when the background read lands. The cue
   * editor's transport (Play, Audition, pre-roll) sets it; nothing else does.
   *
   * `startAt` is air seconds, for the cue editor reloading an edited audition
   * where it was.
   */
  cueLoad(
    track: Track,
    cuePoints: CuePoints | null = null,
    autoplay = false,
    startAt = 0,
  ): void {
    this.cueError = null;
    // A reload of the same track (mode switch, edited audition) keeps its
    // curve and art rather than blanking and refetching them.
    const sameTrack = this.cueTrack?.id === track.id;
    this.cueTrack = track;
    this.cueAppliedPoints = cuePoints;
    this.cueDuration = cuePoints
      ? airDuration({ ...track, cue_points: cuePoints })
      : (track.duration ?? 0);
    this.cueCurrentTime = Math.min(Math.max(0, startAt), this.cueDuration);
    this.cueIsPlaying = autoplay;
    if (!sameTrack) {
      this.loadCueWaveform(track.id);
      this.loadCueCoverArt(track.id);
    }
    void this.cueBackend
      .load(track.id, cuePoints, autoplay, startAt)
      .catch((err) => {
        logger.error("Cue load failed:", err);
        this.cueError = err instanceof Error ? err.message : String(err);
      });
  }

  /** Which audition mode the cue deck is in. */
  get cueMode(): "absolute" | "preview" {
    return this.cueAppliedPoints ? "preview" : "absolute";
  }

  /**
   * Switch audition mode, which reloads the deck: the markers are applied by
   * the player at load time, so there is no way to toggle them on a running
   * source. The reload parks at the top — an operator comparing the two modes
   * should not be made to duck a burst of audio on each toggle.
   */
  setCueMode(mode: "absolute" | "preview"): void {
    const track = this.cueTrack;
    if (!track || this.cueMode === mode) return;
    this.cueLoad(
      track,
      mode === "preview" ? (track.cue_points ?? NO_CUE_POINTS) : null,
    );
  }

  /**
   * What the cue deck is showing right now, so a surface that borrows the deck
   * can hand it back. The cue editor takes one when it opens and restores it on
   * every exit, which is what keeps an unsaved draft from being left armed
   * behind a closed dialog.
   */
  cueSnapshot(): CueSnapshot {
    return { track: this.cueTrack, previewing: this.cueAppliedPoints !== null };
  }

  /**
   * Put the cue deck back the way `cueSnapshot` found it, parked. A *Preview*
   * is re-resolved against the track's current markers rather than the ones
   * captured: after a save, "what it was showing" means the edit just stored.
   */
  cueRestore(snapshot: CueSnapshot): void {
    const track = snapshot.track;
    if (!track) {
      if (this.cueTrack) this.cueStop();
      return;
    }
    const fresh = this.tracks.find((t) => t.id === track.id) ?? track;
    this.cueLoad(
      fresh,
      snapshot.previewing ? (fresh.cue_points ?? NO_CUE_POINTS) : null,
    );
  }

  /**
   * The sub-range of the file the cue deck is showing, as fractions — the whole
   * file in *Absolute* mode, the aired region in *Preview*. `null` when there
   * is nothing to crop against.
   */
  get cueCrop(): { from: number; to: number } | null {
    const track = this.cueTrack;
    if (!track || !this.cueAppliedPoints || !track.duration) return null;
    const cue = resolveCuePoints(this.cueAppliedPoints, track.duration);
    return {
      from: cue.cueIn / track.duration,
      to: cue.cueOut / track.duration,
    };
  }

  /**
   * The sub-range of the file the main deck is airing, as fractions — `null`
   * with nothing on air or no duration to divide by.
   *
   * The deck reports air time, so the bar already measures what plays; without
   * this the curve under it still spanned the whole file, putting the trimmed
   * head and tail on screen and starting the fill somewhere the audio never
   * does.
   */
  get airCrop(): { from: number; to: number } | null {
    const track = this.currentTrack;
    if (!track?.duration) return null;
    const aired = airedTrack(track, this.currentCueOverride);
    const cue = resolveCuePoints(aired.cue_points, track.duration);
    return {
      from: cue.cueIn / track.duration,
      to: cue.cueOut / track.duration,
    };
  }

  /**
   * Persist a track's cue points and adopt the clamped value the backend
   * returns — the one rule lives there, so whatever comes back is the truth.
   * Every copy of the track the UI holds is refreshed, since durations
   * everywhere are derived from these markers.
   */
  async saveCuePoints(id: number, points: CuePoints): Promise<CuePoints> {
    const stored = await api.setCuePoints(id, points);
    this.adoptCuePoints(id, stored);
    this.editingCuePoints = this.applyCuePoints(
      this.editingCuePoints,
      id,
      stored,
    );
    return stored;
  }

  private applyCuePoints(
    track: Track | null,
    id: number,
    points: CuePoints,
  ): Track | null {
    return track && track.id === id ? { ...track, cue_points: points } : track;
  }

  /**
   * Take new markers into every copy of the track the UI holds. Not
   * `currentTrack`: markers apply from the track's next airing, and rewriting
   * them here would make the on-air deck's bar disagree with the audio still
   * coming out of it. An open editor is refreshed only while its draft is
   * untouched.
   */
  private adoptCuePoints(id: number, points: CuePoints): void {
    // The analysis pass reports every result it commits — one per track in the
    // library on a backfill — and rebuilding the rows re-renders the list. A
    // track that is nowhere on screen is dropped before that.
    if (this.tracks.some((t) => t.id === id)) {
      this.tracks = this.tracks.map(
        (t) => this.applyCuePoints(t, id, points) as Track,
      );
    }
    if (this.playlist.some((i) => isTrackItem(i) && i.track.id === id)) {
      this.playlist = this.playlist.map((i) =>
        isTrackItem(i) && i.track.id === id
          ? { ...i, track: { ...i.track, cue_points: points } }
          : i,
      );
    }
    this.cueTrack = this.applyCuePoints(this.cueTrack, id, points);
    // A result landing for the track being edited refreshes a pristine editor.
    // Its draft was built before the analysis existed, so saving it would write
    // those `null`s back over the result and take the trio off automatic for
    // good — from an edit that may only have touched a fade. A draft the
    // operator has already changed stands: what they see is what they save.
    if (!this.cueEditorDirty) {
      this.editingCuePoints = this.applyCuePoints(
        this.editingCuePoints,
        id,
        points,
      );
    }
  }

  /**
   * Take a measured length into every copy of the track the UI holds. Not
   * `currentTrack`, for the reason {@link AppState.adoptCuePoints} leaves it
   * alone: the deck resolved its markers against the length it loaded with.
   *
   * A copy that already holds the length is left as the object it was. The
   * pass reports on every decode, and an overlay re-seeds its form whenever the
   * track it was opened on is replaced — so `editingMetadata`, which shows no
   * length, is never touched, and a cue editor takes one only while its draft
   * is untouched.
   */
  private adoptDuration(id: number, duration: number): void {
    const stale = (track: Track | null): track is Track =>
      track !== null && track.id === id && track.duration !== duration;
    const apply = <T extends Track | null>(track: T): T =>
      stale(track) ? { ...track, duration } : track;
    if (this.tracks.some(stale)) {
      this.tracks = this.tracks.map(apply);
    }
    if (this.playlist.some((i) => isTrackItem(i) && stale(i.track))) {
      this.playlist = this.playlist.map((i) =>
        isTrackItem(i) && stale(i.track)
          ? { ...i, track: { ...i.track, duration } }
          : i,
      );
    }
    this.cueTrack = apply(this.cueTrack);
    if (!this.cueEditorDirty) {
      this.editingCuePoints = apply(this.editingCuePoints);
    }
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
    this.cueAppliedPoints = null;
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
   * Append the cue track to the main playlist.
   * Cue keeps playing — independent transport.
   *
   * A Preview audition that differs from the track's radio edit travels with
   * the item as an override, so what the operator just heard is what airs. An
   * audition matching the radio edit deliberately carries nothing: the item
   * keeps referencing the track, and a later correction still reaches it.
   */
  promoteCueToMain(): void {
    const track = this.cueTrack;
    if (!track) return;
    this.send(api.playlistAdd(track.id, this.cuePromoteOverride));
  }

  /** The override an appended cue item carries, or `null` when it carries none. */
  get cuePromoteOverride(): CuePoints | null {
    const track = this.cueTrack;
    const applied = this.cueAppliedPoints;
    if (!track || !applied) return null;
    return cuePointsEqual(applied, track.cue_points) ? null : applied;
  }

  /**
   * Queue a track next-up carrying `points` for that airing alone — the cue
   * editor's _Use once_, and the ordinary way an item override is authored.
   * Points identical to the track's radio edit carry nothing, so a later
   * correction to the track still reaches the queued airing.
   */
  queueCueDraft(track: Track, points: CuePoints): void {
    const override = cuePointsEqual(points, track.cue_points) ? null : points;
    this.send(api.playlistAddFront(track.id, override));
  }

  /** Whether adding the cue track now would give its item cue points. */
  get cuePromoteCarriesOverride(): boolean {
    return this.cuePromoteOverride !== null;
  }

  /**
   * Set or clear one queued item's override. `null` drops the item back to the
   * track's radio edit.
   */
  setItemCuePoints(index: number, points: CuePoints | null): void {
    if (index < 0 || index >= this.playlist.length) return;
    this.send(api.playlistSetItemCuePoints(index, points));
  }

  // ----- Audio device config -----

  async loadAudioConfig(): Promise<void> {
    try {
      const [devices, main, cue] = await Promise.all([
        api.listAudioDevices(),
        api.getMainDevice(),
        api.getCueDevice(),
      ]);
      this.audioDevices = devices;
      this.mainDevice = main;
      this.cueDevice = cue;
    } catch (err) {
      logger.error("Audio device lookup failed:", err);
    }
  }

  /**
   * Fetch persisted tuning from the backend. Falls back to defaults (already in
   * place) on error so a backend hiccup never leaves the app unusable.
   */
  async loadTuning(): Promise<void> {
    try {
      this.applyTuning(await api.getTuningConfig());
    } catch (err) {
      logger.error("Failed to load tuning config", err);
    }
  }

  /**
   * Persist edited tuning and adopt the backend's clamped result. Cache/player
   * fields only take effect on restart (their worker threads capture them at
   * startup); the renderer-side fields applied here take effect immediately.
   */
  async saveTuning(next: TuningConfig): Promise<void> {
    const was = this.tuning?.autoCue;
    this.applyTuning(await api.setTuningConfig(next));
    const now = this.tuning.autoCue;
    if (now.apply !== was?.apply || now.applyNextStart !== was?.applyNextStart)
      await this.rereadCuePoints();
  }

  /**
   * Switching automatic cue points on or off — the whole trio, or the Next
   * Start alone — changes every derived set the library reports, so every copy
   * the UI holds is stale at once. The queue
   * arrives on its own — the backend re-reads what it holds and pushes a
   * snapshot — but the rest is ours.
   *
   * A stale copy is not merely cosmetic here. The editor saves what it was
   * given, and the backend judges ownership against what it last showed, so
   * handing the editor a pre-flip copy would let a fade-only save read as an
   * edit to the trio: on, it would clear the derived markers for good; off, it
   * would freeze them as the operator's.
   */
  private async rereadCuePoints(): Promise<void> {
    await Promise.all([this.search(), this.loadHealth()]);
    const ids = [this.cueTrack?.id, this.editingCuePoints?.id].filter(
      (id): id is number => id != null,
    );
    if (ids.length === 0) return;
    try {
      for (const fresh of await api.getTracksByIds([...new Set(ids)])) {
        const points = fresh.cue_points ?? NO_CUE_POINTS;
        this.cueTrack = this.applyCuePoints(this.cueTrack, fresh.id, points);
        this.editingCuePoints = this.applyCuePoints(
          this.editingCuePoints,
          fresh.id,
          points,
        );
      }
    } catch (err) {
      logger.error("Re-reading cue points failed:", err);
    }
  }

  /**
   * Adopt a tuning config: store it and rebuild the session-save throttle,
   * since its interval is derived from `sessionSaveThrottleMs`.
   */
  private applyTuning(tuning: TuningConfig): void {
    this.tuning = tuning;
    this.throttledSave.cancel();
    this.throttledSave = throttle(
      () => void this.persistSession(),
      tuning.autoPlaylist.sessionSaveThrottleMs,
    );
  }

  /**
   * Fetch and paint the appearance. Awaited before mount, so the first frame
   * is already the operator's theme. A failure is never fatal: the app starts
   * on the static :root palette and says so in the log.
   */
  async loadAppearance(): Promise<void> {
    try {
      this.applyAppearance(await api.getAppearance());
    } catch (err) {
      logger.error("Failed to load appearance", err);
    }
  }

  /**
   * Re-enumerate the themes directory. Called when the Appearance tab opens
   * and after a reload, never on a timer — nothing repaints unasked.
   */
  async loadThemes(): Promise<void> {
    try {
      this.themes = await api.listThemes();
    } catch (err) {
      logger.error("Failed to list themes", err);
    }
  }

  async setStationName(name: string | null): Promise<void> {
    this.applyAppearance(await api.setStationName(name));
  }

  async setStationImage(slot: ImageSlot, path: string): Promise<void> {
    this.applyAppearance(await api.setStationImage(slot, path));
  }

  async clearStationImage(slot: ImageSlot): Promise<void> {
    this.applyAppearance(await api.clearStationImage(slot));
  }

  async setSpinVinyl(enabled: boolean): Promise<void> {
    this.applyAppearance(await api.setSpinVinyl(enabled));
  }

  /**
   * Adopt the theme the backend resolved and return it, so a caller can read
   * `problem` off the result.
   */
  async setTheme(themeId: string): Promise<Appearance> {
    const next = await api.setTheme(themeId);
    this.applyAppearance(next);
    return next;
  }

  /**
   * Re-read the themes directory and re-resolve the active theme. A theme that
   * has become invalid leaves the colours on screen alone — the backend falls
   * back and reports it in `problem`, mid-show safety over freshness.
   */
  async reloadThemes(): Promise<Appearance> {
    const next = await api.reloadThemes();
    this.applyAppearance(next);
    await this.loadThemes();
    return next;
  }

  /**
   * Paint a resolved appearance: the backend has already merged and validated
   * it, so this only writes it onto &lt;html>.
   */
  private applyAppearance(next: Appearance): void {
    const root = document.documentElement;
    for (const token of this.#appliedTokens) {
      if (!(token in next.tokens)) root.style.removeProperty(token);
    }
    for (const [token, value] of Object.entries(next.tokens)) {
      root.style.setProperty(token, value);
    }
    this.#appliedTokens = Object.keys(next.tokens);

    // `color-scheme` is the only thing native chrome follows — scrollbars, the
    // select popup, form controls. No custom property reaches it.
    root.dataset["themeBase"] = next.base;
    root.style.colorScheme = next.base;

    this.appearance = next;
    savePaintHint(next);

    // A rename should reach the title bar without waiting for a track change.
    const track = this.currentTrack;
    document.title = track
      ? `${track.title} - ${track.artist} | ${this.brandName}`
      : this.brandName;
  }

  async setMainDeviceConfig(device: DeviceRef | null): Promise<void> {
    await api.setMainDevice(device);
    this.mainDevice = device;
  }

  async setCueDeviceConfig(device: DeviceRef | null): Promise<void> {
    await api.setCueDevice(device);
    this.cueDevice = device;
    if (device === null) this.cueStop();
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
    const { state } = result;
    if (result.libraryReset) void this.noteLibraryReset();
    // Master level is fixed at unity: the volume slider left the operator UI,
    // so a persisted value from an older session would be unrecoverable.
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
            : {
                kind: "track" as const,
                id: i.track.id,
                cue_override: i.cue_override ?? null,
              },
        ),
        currentTrackId: this.currentTrack?.id ?? null,
        currentTime: this.currentTime,
        currentCueOverride: this.currentCueOverride,
        autoPlaylistActive: this.autoPlaylistActive,
        autoPlaylistSource: this.autoSource?.id ?? null,
        autoAdvance: this.autoAdvance,
        volume: 1,
        cueVolume: this.cueVolume,
      })
      .catch((err) => {
        logger.error("Session save failed:", err);
      });
  }

  /**
   * Read the configured directories. Failure keeps whatever is already shown
   * and leaves `libraryPathsLoaded` alone, so a backend hiccup never turns a
   * configured library into an invitation to add one.
   */
  async loadLibraryPaths(): Promise<void> {
    try {
      this.libraryPaths = await api.getAllPaths();
      this.libraryPathsLoaded = true;
    } catch (err) {
      logger.error("Library paths load failed:", err);
    }
  }

  async addPath(type: ContentType): Promise<void> {
    const added = await api.addPath(type);
    if (added) await this.loadLibraryPaths();
  }

  async removePath(type: ContentType, p: string): Promise<void> {
    await api.removePath(type, p);
    await this.loadLibraryPaths();
  }

  async loadHealth(): Promise<void> {
    try {
      this.health = await api.libraryHealth();
    } catch (err) {
      logger.error("Library health lookup failed:", err);
    }
  }

  /**
   * Permanently delete the chosen missing tracks. The backend skips any that a
   * scan revived meanwhile, and sends a fresh health report.
   */
  async purgeTracks(ids: number[]): Promise<void> {
    try {
      await api.purgeTracks(ids);
    } catch (err) {
      logger.error("Purge failed:", err);
    }
    await Promise.all([this.loadStats(), this.search()]);
  }

  /**
   * Apply the current thresholds to material already analysed. The rows the
   * backend re-derived on the spot are stale in every copy the UI holds, which
   * is the same problem flipping an apply switch causes — so it is the same
   * re-read. Rows it queued arrive later as `cue-points-ready`, one by one.
   */
  async recalculateAutoCue(): Promise<Recalculated> {
    const done = await api.recalculateAutoCue();
    if (done.updated > 0) await this.rereadCuePoints();
    return done;
  }

  /** Stop a health finding from lighting the badge while it stays as it is. */
  dismissFinding(kind: FindingKind, key = ""): void {
    void api.healthDismiss(kind, key).catch((err) => {
      logger.error("Dismiss failed:", err);
    });
  }

  undismissFinding(kind: FindingKind, key = ""): void {
    void api.healthUndismiss(kind, key).catch((err) => {
      logger.error("Undo dismiss failed:", err);
    });
  }

  checkLibraryNow(): void {
    void api.libraryCheckNow().catch((err) => {
      logger.error("Library check request failed:", err);
    });
  }

  /** Update a track's embedded metadata fields and reflect the change in the local tracks array. */
  async updateTrackMetadata(
    id: number,
    input: Omit<TrackMetadataInput, "id">,
  ): Promise<Track | null> {
    const oldTitle = this.tracks.find((t) => t.id === id)?.title ?? "";
    // Forward only the fields the caller actually set (partial patch); an
    // absent key leaves that column unchanged on the backend.
    const payload: TrackMetadataInput = { id };
    for (const key of METADATA_KEYS) {
      if (input[key] !== undefined) {
        (payload[key] as unknown) = input[key];
      }
    }
    let updatedTrack: Track;
    try {
      updatedTrack = await api.updateTrackMetadata(payload);
    } catch (err) {
      logger.error("updateTrackMetadata failed:", err);
      return null;
    }
    this.adoptTrack(updatedTrack, oldTitle);
    return updatedTrack;
  }

  /**
   * Drop a track's metadata edits and take its tags from the file again.
   * Rejects when the file cannot be read; the edits are then kept.
   */
  async revertTrackTags(id: number): Promise<Track> {
    const oldTitle = this.tracks.find((t) => t.id === id)?.title ?? "";
    const updated = await api.revertTrackTags(id);
    this.adoptTrack(updated, oldTitle);
    if (this.editingMetadata?.id === id) this.editingMetadata = updated;
    return updated;
  }

  retryTagWrite(id: number): void {
    void api.retryTagWrite(id).catch((err) => {
      logger.error("Tag write retry failed:", err);
    });
  }

  dismissTagWrite(id: number): void {
    void api.dismissTagWrite(id).catch((err) => {
      logger.error("Dismiss failed:", err);
    });
  }

  /**
   * Put a track the backend changed in place of the local copies.
   *
   * The row is located by id here rather than by a position captured before
   * the call: `tracks` is replaced wholesale by every {@link AppState.search},
   * and one can resolve while a metadata write is in flight — the write goes
   * through `Health::refresh`, so it is the slower of the two. A remembered
   * index would then overwrite an unrelated row.
   */
  private adoptTrack(updated: Track, oldTitle: string): void {
    const index = this.tracks.findIndex((t) => t.id === updated.id);
    if (index >= 0) {
      this.tracks[index] = updated;
    }
    if (this.currentTrack?.id === updated.id) {
      this.currentTrack = updated;
      if (oldTitle && oldTitle !== updated.title) {
        document.title = `${updated.title} - ${updated.artist} | ${this.brandName}`;
      }
    }
    this.scheduleSave();
  }

  async loadUpdate(): Promise<void> {
    try {
      this.update = await api.updateStatus();
    } catch (err) {
      logger.error("Update status lookup failed:", err);
    }
  }

  async checkForUpdate(): Promise<void> {
    try {
      this.update = await api.updateCheck();
    } catch (err) {
      logger.error("Update check failed:", err);
    }
  }

  /**
   * Install the offered release and restart. The session is flushed first: the
   * restart restores from it, and on Windows the installer ends the process
   * without a close request to save on.
   */
  async installUpdate(): Promise<void> {
    try {
      await this.flushSave();
      await api.updateInstall();
    } catch (err) {
      // The backend has already put the failure in `update:state`.
      logger.error("Update install failed:", err);
    }
  }

  async loadAdmin(): Promise<void> {
    try {
      this.applyAdmin(await api.adminStatus());
    } catch (err) {
      logger.error("Admin status lookup failed:", err);
    }
  }

  /** Resolves to whether the password matched. */
  async unlockAdmin(password: string): Promise<boolean> {
    const ok = await api.adminUnlock(password);
    if (ok) {
      this.applyAdmin({ ...this.admin, unlocked: true });
      this.unlockOpen = false;
    }
    return ok;
  }

  lockAdmin(): void {
    this.applyAdmin({ ...this.admin, unlocked: false });
    void api.adminLock().catch((err) => {
      logger.error("Admin lock failed:", err);
    });
  }

  async setAdminPassword(password: string): Promise<void> {
    await api.adminSetPassword(password);
    this.applyAdmin({ ...this.admin, passwordSet: true, unlocked: true });
  }

  async clearAdminPassword(): Promise<void> {
    await api.adminClearPassword();
    this.applyAdmin({ ...this.admin, passwordSet: false, unlocked: true });
  }

  async setIdleLockMin(minutes: number): Promise<void> {
    await api.adminSetIdleLockMin(minutes);
    await this.loadAdmin();
  }

  /**
   * Adopt a new admin status. Leaving admin mode closes the admin-only
   * dialogs; the cue-point editor stays open, since only its save is gated.
   */
  private applyAdmin(status: AdminStatus): void {
    this.admin = status;
    if (this.isAdmin) {
      this.unlockOpen = false;
    } else {
      this.settingsOpen = false;
      this.editingMetadata = null;
    }
  }

  async scan(): Promise<void> {
    await api.scanLibraries();
  }

  async cancelScan(): Promise<void> {
    await api.cancelScan();
  }

  /** Stop the analysis pass without touching a scan. */
  async cancelAnalysis(): Promise<void> {
    await api.cancelAnalysis();
  }

  /** Show the rebuild notice unless the rescan already finished. */
  private async noteLibraryReset(): Promise<void> {
    this.libraryReset = true;
    try {
      const status = await api.getScanStatus();
      if (status.status !== "running") this.libraryReset = false;
    } catch (err) {
      logger.error("Scan status lookup failed:", err);
    }
  }

  async hydrateScanStatus(): Promise<void> {
    this.scanStatus = await api.getScanStatus();
  }

  async hydrateWaveformStatus(): Promise<void> {
    this.waveformStatus = await api.getWaveformStatus();
  }
}

export const app = new AppState();

/** A span that can run past an hour — queue totals — as H:MM:SS or M:SS. */
export function formatSpan(seconds: number): string {
  if (!isFinite(seconds) || seconds < 3600) return formatTime(seconds);
  const h = Math.floor(seconds / 3600);
  return `${h}:${formatTime(seconds % 3600).padStart(5, "0")}`;
}

export function formatTime(seconds: number): string {
  if (!isFinite(seconds)) return "0:00";
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}
