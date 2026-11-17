/** The host's local time of day as `HH:MM:SS`, on a 24-hour clock. */
export function formatClock(date: Date): string {
  return [date.getHours(), date.getMinutes(), date.getSeconds()]
    .map((part) => String(part).padStart(2, "0"))
    .join(":");
}

/** Milliseconds from `nowMs` to the next whole second, never zero. */
export function msToNextSecond(nowMs: number): number {
  return 1000 - (nowMs % 1000);
}

/**
 * An ETA at Unix `ms` as the clock shows time, or `—` when unknown. `minutes`
 * rounds to the nearest minute and drops the seconds.
 */
export function formatEta(ms: number | null, minutes: boolean): string {
  if (ms === null) return "—";
  if (!minutes) return formatClock(new Date(ms));
  return formatClock(new Date(Math.round(ms / 60_000) * 60_000)).slice(0, 5);
}
