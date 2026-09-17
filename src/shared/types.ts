export type ContentType = "music" | "commercial" | "jingle";

export type SortColumn = "title" | "artist" | "album" | "play_count";
export type SortDir = "asc" | "desc";

export interface SortOption {
  column: SortColumn;
  dir: SortDir;
}

export interface Track {
  id: number;
  title: string;
  artist: string;
  album: string;
  duration: number;
  play_count: number;
  genre?: string | null;
  year?: number | null;
  bpm?: number | null;
  sample_rate?: number | null;
  bitrate?: number | null;
  format?: string;
  /**
   * The track's radio edit. Milliseconds from the start of the file, every
   * marker nullable. Optional here only so test fixtures need not spell it
   * out — the backend sends it on every track.
   */
  cue_points?: CuePoints;
  /**
   * Tag fields edited in the app, as {@link EditedField} bits. A rescan keeps
   * them instead of taking the file's tags.
   */
  edited_fields?: number;
}

/** Bits of `Track.edited_fields`. */
export const EditedField = {
  title: 1,
  artist: 2,
  album: 4,
  genre: 8,
  year: 16,
} as const;

/**
 * Per-track playback markers. `null` means "no adjustment"; the backend
 * resolves each one to a fallback at load time. Clamping is backend-owned:
 * `api.setCuePoints` returns the clamped value to adopt.
 */
export interface CuePoints {
  cue_in_ms: number | null;
  fade_in_ms: number | null;
  fade_out_ms: number | null;
  cue_out_ms: number | null;
  next_start_ms: number | null;
}

export type PlaylistTrackItem = {
  kind: "track";
  track: Track;
  /**
   * Cue points for this one airing, overriding the track's radio edit. Absent
   * — the common case — means the item references the track, so correcting a
   * radio edit corrects every queued airing of it. An all-`null` override is
   * distinct: it says "play the whole file this once".
   */
  cue_override?: CuePoints | null;
};
export type StopMarker = { kind: "stop" };
export type PlaylistItem = PlaylistTrackItem | StopMarker;

export const trackItem = (
  track: Track,
  cue_override: CuePoints | null = null,
): PlaylistTrackItem => ({
  kind: "track",
  track,
  cue_override,
});
export const stopMarker = (): StopMarker => ({ kind: "stop" });
export const isTrackItem = (i: PlaylistItem): i is PlaylistTrackItem =>
  i.kind === "track";
export const isStopMarker = (i: PlaylistItem): i is StopMarker =>
  i.kind === "stop";

export interface LibraryStats {
  totalTracks: number;
  totalArtists: number;
  totalAlbums: number;
  totalHours: number;
  tracksByType: Record<ContentType, number>;
}

/**
 * Partial-update payload for a track's metadata, following JSON Merge Patch
 * (RFC 7396) semantics so it maps 1:1 onto the backend's `Option` fields:
 *
 * - key absent      → leave the field unchanged (`None`)
 * - key present     → set the field to the value, empty string included
 *                     (`Some(value)`)
 * - key present null → clear the nullable field to NULL, genre/year only
 *                     (`Some(None)`)
 */
export interface TrackMetadataInput {
  /** Always present — identifies the track to update. */
  id: number;
  title?: string;
  artist?: string;
  album?: string;
  genre?: string | null;
  year?: number | null;
}

/** A track whose file a scan found gone. */
export interface MissingTrack {
  id: number;
  title: string;
  artist: string;
  /** Where the file was last seen. */
  path: string;
  /** Unix ms. */
  missingSince: number;
  playCount: number;
  hasCuePoints: boolean;
  /** No library path contains `path` any more. */
  outsideRoots: boolean;
}

export interface DuplicateMember {
  track: Track;
  path: string;
  contentType: ContentType;
}

export interface DuplicateGroup {
  key: string;
  dismissed: boolean;
  tracks: DuplicateMember[];
}

/** A track whose file was read but could not be decoded. */
export interface UnreadableTrack {
  track: Track;
  path: string;
  contentType: ContentType;
  error: string;
  /** Unix ms. */
  failedAt: number;
}

/** What a library check found on disk that no scan has applied yet. */
export interface CheckReport {
  /** Unix ms. */
  checkedAt: number;
  new: string[];
  changed: string[];
  gone: string[];
  /** Tracks no library path contains any more. */
  unrooted: string[];
  unreachable: string[];
  partial: string[];
}

export interface HealthReport {
  missing: MissingTrack[];
  missingDismissed: boolean;
  exact: DuplicateGroup[];
  possible: DuplicateGroup[];
  /** Present tracks still waiting to be fingerprinted. */
  unhashed: number;
  /** Present tracks the analysis pass could not decode. */
  unreadable: UnreadableTrack[];
  check: CheckReport | null;
  checkDismissed: boolean;
  /** A library check is running now. */
  checking: boolean;
  /** Edits that could not be written into their file. */
  tagWriteFailures: TagWriteFailure[];
}

/** A metadata edit write-back could not put into the file. */
export interface TagWriteFailure {
  id: number;
  title: string;
  artist: string;
  path: string;
  error: string;
  /** Unix ms. */
  at: number;
}

export type FindingKind = "exact" | "possible" | "missing" | "check";

export interface ScanResult {
  total: number;
  added: number;
  /** Moved files matched back to their tracks. */
  reattached?: number;
  /** Tracks whose file the scan no longer found. */
  missing?: number;
}

export interface NowPlayingConfig {
  webhookUrl: string | null;
  webhookSecret: string | null;
  fileDir: string | null;
  fileEnabled: boolean;
  webhookEnabled: boolean;
}

export interface DeviceRef {
  name: string;
  description: string;
}

/// Playlist interleave cadence.
export interface InterleaveConfig {
  jingleEvery: number;
  commercialEvery: number;
  commercialBucketMultiplier: number;
  commercialBucketMin: number;
}

/// Renderer-side auto-playlist + session tuning. Read by `state.svelte.ts`.
export interface AutoPlaylistConfig {
  autoPlaylistBuffer: number;
  autoPlaylistThreshold: number;
  historyCap: number;
  sessionSaveThrottleMs: number;
  netRetryBackoffsMs: number[];
}

/// Prefetch byte-cache tuning (bytes).
export interface CacheConfig {
  maxCacheBytes: number;
}

/// Audio-player network-resilience timeouts (ms).
export interface PlayerConfig {
  readWatchdogTimeoutMs: number;
  openRetryIntervalMs: number;
  readRetryBackoffsMs: number[];
}

/// Library health tuning.
export interface LibraryConfig {
  /** Minutes between library checks; 0 turns the timer off. */
  checkIntervalMin: number;
  /** Write metadata edits into the file's tags as well. */
  writeTags: boolean;
  /** Seconds a tag write may take before it is reported as failed. */
  tagWriteTimeoutSec: number;
}

/// User-tunable playback behaviour, persisted in `config.json`.
export interface TuningConfig {
  interleave: InterleaveConfig;
  autoPlaylist: AutoPlaylistConfig;
  cache: CacheConfig;
  player: PlayerConfig;
  library: LibraryConfig;
}

export interface DeviceInfo {
  name: string;
  description: string;
  isDefault: boolean;
}

/** Admin mode as the backend reports it. See `docs/admin-mode.md`. */
export interface AdminStatus {
  passwordSet: boolean;
  /** True whenever no password is set. */
  unlocked: boolean;
  idleLockMin: number;
}
