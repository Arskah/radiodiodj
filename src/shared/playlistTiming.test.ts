import { describe, expect, it } from "vitest";
import {
  formatClock,
  formatEta,
  formatStopCounter,
  formatStopDuration,
  occurrenceEta,
  parseStopDuration,
  PlaylistClock,
  sharedDelaySeconds,
  stopElapsedSeconds,
  stopOverPlan,
} from "./playlistTiming";
import type { HoldingStop, PlaylistTiming } from "./types";

const hold: HoldingStop = {
  occurrenceId: "stop-1",
  plannedDurationSeconds: 120,
  elapsedSeconds: 0,
  capturedAtUnixMs: 10_000,
  blockId: "show",
};

const timing: PlaylistTiming = {
  entries: [
    { occurrenceId: "first", baseSecond: 12 },
    { occurrenceId: "second", baseSecond: 62 },
    { occurrenceId: "barrier", baseSecond: null },
  ],
  delaySeconds: 0.7,
  delayStartedAtUnixMs: 10_000,
};

describe("shared playlist display timing", () => {
  it("catches up all rows from a late sample, rounding accumulated delay once", () => {
    expect(sharedDelaySeconds(timing, 13_350)).toBe(4);
    expect(occurrenceEta(timing, "first", 13_350)).toBe(16);
    expect(occurrenceEta(timing, "second", 13_350)).toBe(66);
    expect(occurrenceEta(timing, "barrier", 13_350)).toBeNull();
    expect(occurrenceEta(timing, "removed", 13_350)).toBeNull();
    expect(stopElapsedSeconds(hold, 13_350)).toBeCloseTo(3.35);
  });

  it("freezes fractional pauses without a jump and preserves the next remainder", () => {
    const paused = { ...timing, delaySeconds: 0.2 };
    const frozen = {
      ...paused,
      delaySeconds: 0.65,
      delayStartedAtUnixMs: null,
    };
    expect(occurrenceEta(paused, "first", 10_450)).toBe(12);
    expect(occurrenceEta(frozen, "first", 40_000)).toBe(12);
    const pausedAgain = { ...frozen, delayStartedAtUnixMs: 40_000 };
    expect(occurrenceEta(pausedAgain, "first", 40_450)).toBe(13);
    expect(occurrenceEta(pausedAgain, "second", 40_450)).toBe(63);
  });

  it("keeps the planned stretch fixed until the future delay deadline", () => {
    const planned = { ...timing, delayStartedAtUnixMs: 130_000 };
    expect(occurrenceEta(planned, "first", 20_000)).toBe(12);
    expect(occurrenceEta(planned, "first", 130_000)).toBe(12);
    expect(occurrenceEta(planned, "first", 131_300)).toBe(14);
  });

  it("counts planned and unplanned holds through the deadline", () => {
    expect(formatStopCounter(hold, 10_000)).toBe("−2:00");
    expect(formatStopCounter(hold, 130_000)).toBe("0:00");
    expect(stopOverPlan(hold, 130_000)).toBe(false);
    expect(formatStopCounter(hold, 131_000)).toBe("+0:01");
    expect(stopOverPlan(hold, 131_000)).toBe(true);
    const unplanned = { ...hold, plannedDurationSeconds: null };
    expect(formatStopCounter(unplanned, 131_000)).toBe("2:01");
    expect(stopOverPlan(unplanned, 131_000)).toBe(false);
    expect(
      formatStopCounter({ ...hold, plannedDurationSeconds: 0 }, 10_000),
    ).toBe("0:00");
  });

  it("accepts a blank or whole-second m:ss plan with unbounded minutes", () => {
    expect(parseStopDuration(" ")).toBeNull();
    expect(parseStopDuration("0:00")).toBe(0);
    expect(parseStopDuration(" 120:03 ")).toBe(7203);
    expect(formatStopDuration(7203)).toBe("120:03");
    expect(formatStopDuration(null)).toBe("");
    for (const invalid of ["1", "-1:00", "1:60", "1:1", "1:00.5", "1:00:00"])
      expect(parseStopDuration(invalid)).toBeUndefined();
  });

  it("formats local wall time across midnight without adding dates to ETA rows", () => {
    const before = new Date(2026, 9, 18, 23, 59, 59).getTime();
    expect(formatClock(before)).toBe("23:59:59");
    expect(formatEta(before / 1000 + 2)).toBe("00:00:01");
    expect(formatEta(null)).toBe("—");
  });
});

describe("monotonic playlist clock", () => {
  it("samples elapsed time without drift or a wall-clock jump until reanchored", () => {
    let wall = 10_000;
    let monotonic = 0;
    const clock = new PlaylistClock({
      wallNow: () => wall,
      monotonicNow: () => monotonic,
    });
    monotonic = 3_350;
    wall = 13_350;
    expect(clock.now()).toBe(13_350);
    expect(clock.needsReanchor()).toBe(false);
    wall += 60_000;
    expect(clock.now()).toBe(13_350);
    expect(clock.needsReanchor()).toBe(true);
    expect(clock.reanchor()).toBe(73_350);
    expect(clock.needsReanchor()).toBe(false);
    monotonic += 250;
    expect(clock.now()).toBe(73_600);
  });
});
