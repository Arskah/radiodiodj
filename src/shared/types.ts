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
}

export type PlaylistTrackItem = { kind: "track"; track: Track };
export type StopMarker = { kind: "stop" };
export type PlaylistItem = PlaylistTrackItem | StopMarker;

export const trackItem = (track: Track): PlaylistTrackItem => ({
  kind: "track",
  track,
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

/** Serializable form for tracking which metadata fields were changed
 * by the user. Only non-Empty values are sent to the backend so empty
 * strings can clear a field without requiring `undefined`. */
export interface TrackMetadataInput {
  /** Always present — identifies the track to update. */
  id: number;
  title: string;
  artist: string;
  album: string;
  genre: string | null;
  year: number | null;
}

export interface ScanResult {
  total: number;
  added: number;
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

export interface DeviceInfo {
  name: string;
  description: string;
  isDefault: boolean;
}
