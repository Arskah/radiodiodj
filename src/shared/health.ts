import type { CheckReport, HealthReport } from "./types";

/** A check found something the next scan would change. */
export function checkHasChanges(check: CheckReport | null): boolean {
  return (
    check !== null &&
    check.new.length +
      check.changed.length +
      check.gone.length +
      check.unrooted.length >
      0
  );
}

/**
 * How many findings want the operator's attention: what the Settings button
 * and the Library health tab show. Missing tracks count once, not per track,
 * so removing a library path does not put hundreds on the button. An
 * unreachable path cannot be dismissed; it clears when the share is back. A
 * failed tag write counts until it is retried or dismissed.
 */
export function healthAttention(report: HealthReport): number {
  let count = 0;
  if (report.missing.length > 0 && !report.missingDismissed) count += 1;
  count += report.exact.filter((g) => !g.dismissed).length;
  count += report.possible.filter((g) => !g.dismissed).length;
  if (checkHasChanges(report.check) && !report.checkDismissed) count += 1;
  count += report.check?.unreachable.length ?? 0;
  count += report.tagWriteFailures.length;
  return count;
}

/** "just now", "5 min ago", "3 h ago", "2 d ago". */
export function formatAgo(ms: number, now = Date.now()): string {
  const minutes = Math.floor((now - ms) / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  return `${Math.floor(hours / 24)} d ago`;
}

export function plural(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}
