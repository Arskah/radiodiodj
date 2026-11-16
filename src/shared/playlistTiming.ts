import type { HoldingStop, PlaylistTiming } from "./types";

/** Injectable wall and monotonic time sources, sampled by one shared clock. */
export interface ClockSource {
  wallNow(): number;
  monotonicNow(): number;
}

/** Browser clock sources; elapsed time is independent of host-clock edits. */
export const systemClock: ClockSource = {
  wallNow: () => Date.now(),
  monotonicNow: () => performance.now(),
};

/** Wall-clock anchor advanced by monotonic elapsed time between corrections. */
export class PlaylistClock {
  private wallAnchor: number;
  private monotonicAnchor: number;

  constructor(private source: ClockSource = systemClock) {
    this.wallAnchor = source.wallNow();
    this.monotonicAnchor = source.monotonicNow();
  }

  /** Sample elapsed time rather than counting timer callbacks. */
  now(): number {
    return this.wallAnchor + this.source.monotonicNow() - this.monotonicAnchor;
  }

  /** A deliberate host-clock correction requires a fresh backend schedule. */
  needsReanchor(): boolean {
    return Math.abs(this.source.wallNow() - this.now()) > 1000;
  }

  /** Accept the host wall clock after its matching schedule has arrived. */
  reanchor(): number {
    this.wallAnchor = this.source.wallNow();
    this.monotonicAnchor = this.source.monotonicNow();
    return this.wallAnchor;
  }
}

/** Round the accumulated delay once, identically for every schedule entry. */
export function sharedDelaySeconds(
  timing: PlaylistTiming,
  clockNow: number,
): number {
  const running =
    timing.delayStartedAtUnixMs === null
      ? 0
      : Math.max(0, (clockNow - timing.delayStartedAtUnixMs) / 1000);
  return Math.floor(timing.delaySeconds + running);
}

/** Resolve an occurrence's ETA without reconstructing the backend schedule. */
export function occurrenceEta(
  timing: PlaylistTiming,
  occurrenceId: string,
  clockNow: number,
): number | null {
  const base = timing.entries.find(
    (entry) => entry.occurrenceId === occurrenceId,
  )?.baseSecond;
  return base == null ? null : base + sharedDelaySeconds(timing, clockNow);
}

/** Local wall time, including seconds and preserving midnight rollover. */
export function formatClock(unixMs: number): string {
  const date = new Date(unixMs);
  return [date.getHours(), date.getMinutes(), date.getSeconds()]
    .map((part) => String(part).padStart(2, "0"))
    .join(":");
}

/** An unknown schedule stays visibly unknown. */
export function formatEta(unixSeconds: number | null): string {
  return unixSeconds === null ? "—" : formatClock(unixSeconds * 1000);
}

const minutesSeconds = (seconds: number): string =>
  `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
const maxPlannedStopSeconds = 0xffff_ffff;

/** The editable plan uses minutes without an upper bound. */
export function formatStopDuration(seconds: number | null | undefined): string {
  return seconds == null ? "" : minutesSeconds(seconds);
}

/** `undefined` rejects invalid input; `null` clears the optional plan. */
export function parseStopDuration(text: string): number | null | undefined {
  const value = text.trim();
  if (value === "") return null;
  if (/^\d+$/.test(value)) {
    const seconds = Number(value) * 60;
    return Number.isSafeInteger(seconds) && seconds <= maxPlannedStopSeconds
      ? seconds
      : undefined;
  }
  const match = /^(\d+):([0-5]\d)$/.exec(value);
  if (!match) return undefined;
  const seconds = Number(match[1]) * 60 + Number(match[2]);
  return Number.isSafeInteger(seconds) && seconds <= maxPlannedStopSeconds
    ? seconds
    : undefined;
}

/** Continue the captured hold using monotonic-anchored display time. */
export function stopElapsedSeconds(
  hold: HoldingStop,
  clockNow: number,
): number {
  return (
    hold.elapsedSeconds + Math.max(0, (clockNow - hold.capturedAtUnixMs) / 1000)
  );
}

/** A planned deadline warns only once the elapsed hold exceeds it. */
export function stopOverPlan(hold: HoldingStop, clockNow: number): boolean {
  return (
    hold.plannedDurationSeconds !== null &&
    stopElapsedSeconds(hold, clockNow) > hold.plannedDurationSeconds
  );
}

/** Planned holds count down through zero; unplanned holds count elapsed time. */
export function formatStopCounter(hold: HoldingStop, clockNow: number): string {
  const elapsed = Math.floor(stopElapsedSeconds(hold, clockNow));
  if (hold.plannedDurationSeconds === null) return minutesSeconds(elapsed);
  const difference = elapsed - hold.plannedDurationSeconds;
  const sign = difference < 0 ? "−" : difference > 0 ? "+" : "";
  return sign + minutesSeconds(Math.abs(difference));
}
