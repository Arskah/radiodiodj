/**
 * What the panel says after a saved playlist is appended to the playlist. Only
 * an unmatched entry is ever skipped. See `docs/saved-playlists.md`.
 */
export function appendMessage(added: number, skipped: number): string {
  const tracks = `${added} ${added === 1 ? "track" : "tracks"}`;
  if (skipped === 0) return `Added ${tracks}`;
  if (added === 0) return `Nothing to add: ${skipped} unmatched`;
  return `Added ${tracks}, skipped ${skipped} unmatched`;
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

/** What the panel says once a file has become a saved playlist. */
export function importMessage(
  name: string,
  entries: number,
  missing: number,
): string {
  return `Imported “${name}”: ${sizeLabel(entries, missing)}`;
}

/** How a saved playlist's size reads in the list: `12 tracks · 2 missing`. */
export function sizeLabel(entries: number, missing: number): string {
  const tracks = `${entries} ${entries === 1 ? "track" : "tracks"}`;
  return missing > 0 ? `${tracks} · ${missing} missing` : tracks;
}
