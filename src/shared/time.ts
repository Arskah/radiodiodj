/** Clock formatting with no state behind it, so anything may import it. */

export function formatTime(seconds: number): string {
  if (!isFinite(seconds)) return "0:00";
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}

/** A span that can run past an hour — queue totals — as H:MM:SS or M:SS. */
export function formatSpan(seconds: number): string {
  if (!isFinite(seconds) || seconds < 3600) return formatTime(seconds);
  const h = Math.floor(seconds / 3600);
  return `${h}:${formatTime(seconds % 3600).padStart(5, "0")}`;
}
