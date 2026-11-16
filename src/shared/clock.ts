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
