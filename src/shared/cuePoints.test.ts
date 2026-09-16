import { describe, expect, it } from "vitest";
import {
  CUE_MARKERS,
  NO_CUE_POINTS,
  airDuration,
  airedTrack,
  cueMarkerPositions,
  cuePointsEqual,
  hasCuePoints,
  isTrimmed,
  resolveCuePoints,
} from "./cuePoints";
import type { CuePoints, Track } from "./types";

const points = (p: Partial<CuePoints>): CuePoints => ({
  ...NO_CUE_POINTS,
  ...p,
});

const track = (duration: number, p?: Partial<CuePoints>): Track => ({
  id: 1,
  title: "t",
  artist: "a",
  album: "al",
  duration,
  play_count: 0,
  ...(p ? { cue_points: points(p) } : {}),
});

describe("hasCuePoints", () => {
  it("is false for an untouched track", () => {
    expect(hasCuePoints(undefined)).toBe(false);
    expect(hasCuePoints(null)).toBe(false);
    expect(hasCuePoints(NO_CUE_POINTS)).toBe(false);
  });

  it("is true as soon as any one marker is set", () => {
    for (const { key } of CUE_MARKERS) {
      expect(hasCuePoints(points({ [key]: 1000 }))).toBe(true);
    }
  });

  it("counts a zero as set — cue in at 0 is a real decision", () => {
    expect(hasCuePoints(points({ cue_in_ms: 0 }))).toBe(true);
  });
});

describe("resolveCuePoints", () => {
  it("resolves a bare track to the whole file", () => {
    expect(resolveCuePoints(undefined, 200)).toEqual({
      cueIn: 0,
      fadeIn: 0,
      fadeOut: 200,
      cueOut: 200,
      nextStart: 200,
    });
  });

  it("applies each documented null fallback", () => {
    const cue = resolveCuePoints(
      points({ cue_in_ms: 10_000, cue_out_ms: 180_000 }),
      200,
    );
    expect(cue.fadeIn).toBe(cue.cueIn);
    expect(cue.fadeOut).toBe(cue.cueOut);
    expect(cue.nextStart).toBe(cue.cueOut);
  });

  it("converts stored milliseconds to seconds", () => {
    const cue = resolveCuePoints(
      points({
        cue_in_ms: 10_500,
        fade_in_ms: 12_000,
        fade_out_ms: 170_000,
        cue_out_ms: 180_000,
        next_start_ms: 176_000,
      }),
      200,
    );
    expect(cue).toEqual({
      cueIn: 10.5,
      fadeIn: 12,
      fadeOut: 170,
      cueOut: 180,
      nextStart: 176,
    });
  });

  it("treats an unknown file duration as zero rather than guessing an end", () => {
    const cue = resolveCuePoints(points({ cue_in_ms: 5_000 }), 0);
    expect(cue.cueIn).toBe(5);
    expect(cue.cueOut).toBe(0);
  });
});

describe("airDuration", () => {
  it("is the file length when nothing is trimmed", () => {
    expect(airDuration(track(200))).toBe(200);
  });

  it("is the span between the cue points", () => {
    expect(
      airDuration(track(200, { cue_in_ms: 10_000, cue_out_ms: 180_000 })),
    ).toBe(170);
  });

  it("never goes negative on a degenerate region", () => {
    // Backend clamping makes this unreachable; the UI still must not render a
    // negative time if a stale row ever arrives.
    expect(
      airDuration(track(200, { cue_in_ms: 100_000, cue_out_ms: 10_000 })),
    ).toBe(0);
  });
});

describe("isTrimmed", () => {
  it("is false without cue points", () => {
    expect(isTrimmed(track(200))).toBe(false);
  });

  it("is false when the markers happen to span the whole file", () => {
    expect(isTrimmed(track(200, { cue_in_ms: 0, cue_out_ms: 200_000 }))).toBe(
      false,
    );
  });

  it("is true once the aired span is shorter", () => {
    expect(isTrimmed(track(200, { cue_out_ms: 180_000 }))).toBe(true);
  });
});

describe("cueMarkerPositions", () => {
  it("draws only the markers that are set", () => {
    const drawn = cueMarkerPositions(
      points({ cue_in_ms: 20_000, cue_out_ms: 180_000 }),
      200,
    );
    expect(drawn.map((m) => m.id)).toEqual(["cue_in_ms", "cue_out_ms"]);
  });

  it("positions each marker as a fraction of the file", () => {
    const [marker] = cueMarkerPositions(points({ cue_in_ms: 50_000 }), 200);
    expect(marker.at).toBe(0.25);
    expect(marker.kind).toBe("cue-in");
  });

  it("draws nothing without a file duration to scale against", () => {
    expect(cueMarkerPositions(points({ cue_in_ms: 1000 }), 0)).toEqual([]);
  });
});

describe("airedTrack", () => {
  it("is the track itself when the item carries no override", () => {
    const t = track(200, { cue_out_ms: 180_000 });
    expect(airedTrack(t, null)).toBe(t);
    expect(airedTrack(t, undefined)).toBe(t);
  });

  it("swaps in the override, leaving the stored track untouched", () => {
    const t = track(200, { cue_out_ms: 180_000 });
    const aired = airedTrack(t, points({ cue_out_ms: 60_000 }));
    expect(airDuration(aired)).toBe(60);
    expect(airDuration(t)).toBe(180);
  });

  // An all-null override is "play the whole file this once", not "no override".
  it("an all-null override restores the whole file", () => {
    const aired = airedTrack(track(200, { cue_out_ms: 60_000 }), NO_CUE_POINTS);
    expect(airDuration(aired)).toBe(200);
  });
});

describe("cuePointsEqual", () => {
  it("counts an absent marker set as all-null", () => {
    expect(cuePointsEqual(undefined, null)).toBe(true);
    expect(cuePointsEqual(NO_CUE_POINTS, undefined)).toBe(true);
    expect(cuePointsEqual(points({ cue_in_ms: 1000 }), null)).toBe(false);
  });

  it("compares every marker", () => {
    for (const { key } of CUE_MARKERS) {
      expect(
        cuePointsEqual(points({ [key]: 1000 }), points({ [key]: 1000 })),
      ).toBe(true);
      expect(
        cuePointsEqual(points({ [key]: 1000 }), points({ [key]: 2000 })),
      ).toBe(false);
    }
  });
});
