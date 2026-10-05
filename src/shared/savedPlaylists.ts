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

/** How a saved playlist's size reads in the list: `12 tracks · 2 missing`. */
export function sizeLabel(entries: number, missing: number): string {
  const tracks = `${entries} ${entries === 1 ? "track" : "tracks"}`;
  return missing > 0 ? `${tracks} · ${missing} missing` : tracks;
}
