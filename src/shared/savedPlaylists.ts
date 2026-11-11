import type { PlaylistItem, SavedEntry, SavedPlaylistKind } from "./types";

/** The kind shown when saving the remaining queue. */
export function kindForQueue(
  items: readonly PlaylistItem[],
): SavedPlaylistKind {
  return items.some((item) => item.kind === "stop")
    ? "prepared_show"
    : "playlist";
}

/** Stops must be removed before a prepared show becomes a playlist. */
export function canSetKind(kind: SavedPlaylistKind, stops: number): boolean {
  return kind === "prepared_show" || stops === 0;
}

/** The label used in the shared saved-list selector and rows. */
export function kindLabel(kind: SavedPlaylistKind): string {
  return kind === "prepared_show" ? "Prepared show" : "Playlist";
}

/**
 * What the panel says after a saved playlist is appended to the playlist. Only
 * an unmatched entry is ever skipped. See `docs/saved-playlists.md`.
 */
export function appendMessage(
  added: number,
  skipped: number,
  stops = 0,
): string {
  const tracks = `${added} ${added === 1 ? "track" : "tracks"}`;
  const included =
    stops > 0
      ? `${tracks}, ${stops} ${stops === 1 ? "Stop" : "Stops"}`
      : tracks;
  if (skipped === 0) return `Added ${included}`;
  if (added === 0 && stops === 0) return `Nothing to add: ${skipped} unmatched`;
  return `Added ${included}, skipped ${skipped} unmatched`;
}

const words = (text: string): string[] =>
  text
    .toLowerCase()
    .split(/[^\p{L}\p{N}]+/u)
    .filter((word) => word !== "");

/**
 * Whether a search matches a row of text fields, by the library search's rule:
 * every word typed is the start of some word in the fields. An empty search
 * matches everything.
 */
export function matchesSearch(query: string, fields: string[]): boolean {
  const wanted = words(query);
  if (wanted.length === 0) return true;
  const have = fields.flatMap(words);
  return wanted.every((prefix) => have.some((word) => word.startsWith(prefix)));
}

/** What an open saved playlist can be sorted by. `position` is its own order. */
export type EntrySort =
  "position" | "title" | "artist" | "album" | "plays" | "duration";

/** One entry as the list shows it, with its place in the saved playlist. */
export interface EntryRow {
  position: number;
  title: string;
  artist: string;
  album: string;
  plays: number;
  duration: number;
}

/**
 * Rows in the order a sort asks for. Text sorts without regard to case or
 * accents; rows that tie keep the saved playlist's own order, whichever way
 * the sort runs. Returns a new array.
 */
export function sortEntries<T extends EntryRow>(
  rows: readonly T[],
  by: EntrySort,
  dir: "asc" | "desc",
): T[] {
  const sign = dir === "asc" ? 1 : -1;
  const compare = (a: T, b: T): number => {
    if (by === "position") return a.position - b.position;
    if (by === "duration") return a.duration - b.duration;
    if (by === "plays") return a.plays - b.plays;
    return a[by].localeCompare(b[by], undefined, { sensitivity: "base" });
  };
  return [...rows].sort(
    (a, b) => sign * compare(a, b) || a.position - b.position,
  );
}

/**
 * What _Find in library_ first searches for: the entry's artist and title,
 * less anything in brackets. Every word of a search has to match, and
 * `(Radio Edit)` or `[2011 Remaster]` is what two copies of a song differ by.
 */
export function findQuery(entry: SavedEntry): string {
  if (entry.kind === "stop") return "";
  const artist = entry.track?.artist ?? entry.artist;
  const title = entry.track?.title ?? entry.title;
  return `${artist} ${title}`
    .replace(/\([^)]*\)|\[[^\]]*\]/g, " ")
    .split(/\s+/)
    .filter((word) => word !== "")
    .join(" ");
}

/** What the panel says once a file has become a saved playlist. */
export function importMessage(
  name: string,
  entries: number,
  missing: number,
  stops = 0,
): string {
  return `Imported “${name}”: ${sizeLabel(entries, missing, stops)}`;
}

/** How a saved list's size reads, with Stops distinct from tracks. */
export function sizeLabel(entries: number, missing: number, stops = 0): string {
  const trackCount = entries - stops;
  const tracks = `${trackCount} ${trackCount === 1 ? "track" : "tracks"}`;
  const parts = [tracks];
  if (stops > 0) parts.push(`${stops} ${stops === 1 ? "Stop" : "Stops"}`);
  if (missing > 0) parts.push(`${missing} missing`);
  return parts.join(" · ");
}
