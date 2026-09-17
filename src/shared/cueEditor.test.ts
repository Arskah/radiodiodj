import { describe, expect, it } from "vitest";
import {
  editorFrame,
  envelopePoints,
  followFrame,
  followNeedsPage,
  formatClock,
  formatCueTime,
  gainAt,
  markerBounds,
  needsReframe,
  nudge,
  nudgeStep,
  panFrame,
  parseCueTime,
  playheadFile,
  preRollTarget,
  rebucket,
  regionFrame,
  regionOutside,
  resizeFrame,
  reloadStartAt,
  setMarker,
  stackFlags,
} from "./cueEditor";
import { NO_CUE_POINTS } from "./cuePoints";
import type { CuePoints } from "./types";

const points = (p: Partial<CuePoints>): CuePoints => ({
  ...NO_CUE_POINTS,
  ...p,
});

const FILE = 1235.518; // the 20:35 track from the recording

describe("regionFrame", () => {
  it("gives a short region five seconds either side", () => {
    const f = regionFrame(
      points({ cue_in_ms: 100_000, cue_out_ms: 108_000 }),
      FILE,
    );
    expect(f).toEqual({ from: 95, to: 113 });
  });

  it("gives a long region fifteen percent either side", () => {
    const f = regionFrame(
      points({ cue_in_ms: 100_000, cue_out_ms: 700_000 }),
      FILE,
    );
    expect(f).toEqual({ from: 10, to: 790 });
  });

  it("stays inside the file", () => {
    const f = regionFrame(points({ cue_in_ms: 1000 }), FILE);
    expect(f.from).toBe(0);
    expect(f.to).toBe(FILE);
  });

  it("frames an open end against the end of the file", () => {
    const f = regionFrame(points({ cue_in_ms: 1_200_000 }), FILE);
    // 35.5 s region: 15 % beats the 5 s floor.
    expect(f.from).toBeCloseTo(1200 - 35.518 * 0.15);
    expect(f.to).toBe(FILE);
  });
});

describe("followFrame", () => {
  it("centres on the playhead", () => {
    expect(followFrame(100, FILE)).toEqual({ from: 70, to: 130 });
  });

  it("slides rather than shrinks at either end", () => {
    expect(followFrame(3, FILE)).toEqual({ from: 0, to: 60 });
    const end = followFrame(FILE - 1, FILE);
    expect(end.to).toBeCloseTo(FILE);
    expect(end.to - end.from).toBeCloseTo(60);
  });

  it("shows all of a file shorter than the window", () => {
    expect(followFrame(5, 12)).toEqual({ from: 0, to: 12 });
  });
});

describe("editorFrame", () => {
  it("follows the playhead until a region exists", () => {
    expect(editorFrame(points({}), FILE, 100)).toEqual({ from: 70, to: 130 });
    expect(editorFrame(points({ fade_in_ms: 5000 }), FILE, null)).toEqual({
      from: 0,
      to: 60,
    });
  });

  it("frames the region once Cue In or Cue Out is set", () => {
    expect(
      editorFrame(points({ cue_in_ms: 100_000, cue_out_ms: 108_000 }), FILE, 5),
    ).toEqual({ from: 95, to: 113 });
  });
});

describe("regionOutside / followNeedsPage", () => {
  const frame = { from: 98, to: 110 };

  it("flags a Cue In or Cue Out out of view", () => {
    const inView = points({ cue_in_ms: 100_000, cue_out_ms: 108_000 });
    expect(regionOutside(frame, inView, FILE)).toBe(false);
    expect(regionOutside(frame, { ...inView, cue_in_ms: 97_000 }, FILE)).toBe(
      true,
    );
    expect(regionOutside(frame, { ...inView, cue_out_ms: 111_000 }, FILE)).toBe(
      true,
    );
  });

  it("turns the page near the right edge, or when the playhead left", () => {
    const f = { from: 0, to: 30 };
    expect(followNeedsPage(f, 10, FILE)).toBe(false);
    expect(followNeedsPage(f, 28, FILE)).toBe(true);
    expect(followNeedsPage(f, 40, FILE)).toBe(true);
    expect(followNeedsPage({ from: 5, to: 35 }, 1, FILE)).toBe(true);
  });

  it("does not turn past the end of the file", () => {
    expect(followNeedsPage({ from: FILE - 30, to: FILE }, FILE - 1, FILE)).toBe(
      false,
    );
  });
});

describe("resizeFrame / panFrame", () => {
  const frame = { from: 100, to: 130 };

  it("moves the edge that was grabbed", () => {
    expect(resizeFrame(frame, "from", 110, FILE)).toEqual({
      from: 110,
      to: 130,
    });
    expect(resizeFrame(frame, "to", 140, FILE)).toEqual({
      from: 100,
      to: 140,
    });
  });

  it("never drags an edge past the other", () => {
    expect(resizeFrame(frame, "from", 200, FILE)).toEqual({
      from: 129.5,
      to: 130,
    });
    expect(resizeFrame(frame, "to", 0, FILE)).toEqual({ from: 100, to: 100.5 });
  });

  it("keeps the edges inside the file", () => {
    expect(resizeFrame(frame, "from", -10, FILE).from).toBe(0);
    expect(resizeFrame(frame, "to", 1e6, FILE).to).toBe(FILE);
  });

  it("pans without changing the width", () => {
    expect(panFrame(frame, 200, FILE)).toEqual({ from: 200, to: 230 });
  });

  it("stops panning at either end of the file", () => {
    expect(panFrame(frame, -50, FILE)).toEqual({ from: 0, to: 30 });
    const end = panFrame(frame, 1e6, FILE);
    expect(end.to).toBeCloseTo(FILE);
    expect(end.to - end.from).toBeCloseTo(30);
  });

  it("shows the whole of a file shorter than the frame", () => {
    expect(panFrame({ from: 0, to: 30 }, 5, 12)).toEqual({ from: 0, to: 30 });
  });
});

describe("needsReframe", () => {
  it("zooms in once Cue Out follows Cue In", () => {
    // Cue In alone frames everything up to the end of the file.
    const inOnly = points({ cue_in_ms: 100_000 });
    const frame = regionFrame(inOnly, FILE);
    const both = { ...inOnly, cue_out_ms: 110_000 };
    expect(needsReframe(frame, both, FILE)).toBe(true);
  });

  it("holds still while a marker is nudged", () => {
    const d = points({ cue_in_ms: 100_000, cue_out_ms: 110_000 });
    const frame = regionFrame(d, FILE);
    expect(needsReframe(frame, { ...d, cue_out_ms: 110_010 }, FILE)).toBe(
      false,
    );
    expect(needsReframe(frame, { ...d, cue_in_ms: 99_000 }, FILE)).toBe(false);
  });

  it("moves when a marker leaves the frame", () => {
    const d = points({ cue_in_ms: 100_000, cue_out_ms: 110_000 });
    const frame = regionFrame(d, FILE);
    expect(needsReframe(frame, { ...d, cue_out_ms: 116_000 }, FILE)).toBe(true);
  });
});

describe("markerBounds / setMarker", () => {
  it("stops at the nearest set neighbours", () => {
    const d = points({
      cue_in_ms: 1000,
      fade_out_ms: 9000,
      cue_out_ms: 10_000,
    });
    expect(markerBounds(d, "fade_in_ms", FILE)).toEqual({ lo: 1000, hi: 9000 });
    expect(setMarker(d, "fade_in_ms", 9500, FILE).fade_in_ms).toBe(9000);
    expect(setMarker(d, "fade_in_ms", 500, FILE).fade_in_ms).toBe(1000);
  });

  it("skips unset neighbours, so an unset fade never pins Cue In", () => {
    const d = points({ cue_in_ms: 1000, cue_out_ms: 10_000 });
    expect(markerBounds(d, "cue_in_ms", FILE)).toEqual({ lo: 0, hi: 10_000 });
    expect(setMarker(d, "cue_in_ms", 5000, FILE).cue_in_ms).toBe(5000);
  });

  it("keeps a marker inside the file", () => {
    const d = points({});
    expect(setMarker(d, "cue_out_ms", 9e9, FILE).cue_out_ms).toBe(1_235_518);
    expect(setMarker(d, "cue_in_ms", -5, FILE).cue_in_ms).toBe(0);
  });

  it("leaves Next Start free of the fades and the out-point", () => {
    const d = points({ cue_in_ms: 5000, cue_out_ms: 10_000 });
    expect(markerBounds(d, "next_start_ms", FILE)).toEqual({
      lo: 0,
      hi: 1_235_518,
    });
  });

  it("leaves the top open when the duration is unknown", () => {
    expect(markerBounds(points({}), "cue_out_ms", 0).hi).toBe(Infinity);
  });

  it("rounds to whole milliseconds", () => {
    expect(setMarker(points({}), "cue_in_ms", 1234.6, FILE).cue_in_ms).toBe(
      1235,
    );
  });
});

describe("nudge", () => {
  it("moves a set marker by the step", () => {
    const d = points({ cue_in_ms: 1000 });
    expect(nudge(d, "cue_in_ms", 10, FILE).cue_in_ms).toBe(1010);
    expect(nudge(d, "cue_in_ms", -100, FILE).cue_in_ms).toBe(900);
  });

  it("starts an unset marker from its fallback", () => {
    const d = points({ cue_in_ms: 1000 });
    expect(nudge(d, "fade_in_ms", 10, FILE).fade_in_ms).toBe(1010);
    expect(nudge(points({}), "cue_out_ms", -10, FILE).cue_out_ms).toBe(
      1_235_508,
    );
  });

  it("stops at a neighbour", () => {
    const d = points({ cue_in_ms: 1000, fade_in_ms: 1005 });
    expect(nudge(d, "cue_in_ms", 10, FILE).cue_in_ms).toBe(1005);
  });

  it("picks the step from the modifiers", () => {
    expect(nudgeStep({ shiftKey: false, altKey: false })).toBe(10);
    expect(nudgeStep({ shiftKey: true, altKey: false })).toBe(100);
    expect(nudgeStep({ shiftKey: true, altKey: true })).toBe(1000);
  });
});

describe("parseCueTime / formatCueTime", () => {
  it("reads m:ss.mmm", () => {
    expect(parseCueTime("8:55.152")).toBe(535_152);
    expect(parseCueTime("8:55.1")).toBe(535_100);
    expect(parseCueTime("20:35")).toBe(1_235_000);
  });

  it("reads plain seconds and milliseconds", () => {
    expect(parseCueTime("535.152")).toBe(535_152);
    expect(parseCueTime(" 535 ")).toBe(535_000);
    expect(parseCueTime("535152ms")).toBe(535_152);
    expect(parseCueTime("535152 MS")).toBe(535_152);
  });

  it("rejects anything else", () => {
    for (const bad of ["", "abc", "8:75", "1:2:3", "-5", "5.1234", "8:5x"]) {
      expect(parseCueTime(bad), bad).toBeNull();
    }
  });

  it("round-trips", () => {
    for (const ms of [0, 7, 535_152, 1_235_518, 3_600_001]) {
      expect(parseCueTime(formatCueTime(ms))).toBe(ms);
    }
    expect(formatCueTime(535_152)).toBe("8:55.152");
    expect(formatCueTime(61_000)).toBe("1:01.000");
  });

  it("formats the transport clock to a tenth", () => {
    expect(formatClock(535.16)).toBe("8:55.2");
    expect(formatClock(59.96)).toBe("1:00.0");
    expect(formatClock(-1)).toBe("0:00.0");
  });
});

describe("rebucket", () => {
  const src = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];

  it("takes the loudest bucket each output bucket covers", () => {
    expect(rebucket(src, 1, 0, 10, 5)).toEqual([2, 4, 6, 8, 10]);
  });

  it("stretches a narrow window over many output buckets", () => {
    expect(rebucket(src, 1, 2, 4, 4)).toEqual([3, 3, 4, 4]);
  });

  it("reads silence outside the curve", () => {
    expect(rebucket(src, 1, 8, 12, 4)).toEqual([9, 10, 0, 0]);
    expect(rebucket(src, 1, -2, 0, 2)).toEqual([0, 0]);
  });

  it("returns silence for a degenerate request", () => {
    expect(rebucket(src, 1, 5, 5, 3)).toEqual([0, 0, 0]);
    expect(rebucket([], 1, 0, 5, 2)).toEqual([0, 0]);
    expect(rebucket(src, 1, 0, 10, 0)).toEqual([]);
  });
});

describe("playheadFile / reloadStartAt", () => {
  const draft = points({ cue_in_ms: 100_000, cue_out_ms: 160_000 });

  it("reads raw playback as file time", () => {
    expect(playheadFile(null, FILE, 42)).toBe(42);
  });

  it("puts Cue In back onto an audition's air time", () => {
    expect(playheadFile(draft, FILE, 5)).toBe(105);
  });

  it("resumes an edited audition at the same file position", () => {
    const moved = { ...draft, cue_in_ms: 102_000 };
    const at = playheadFile(draft, FILE, 20);
    expect(reloadStartAt(at, moved, FILE)).toBe(18);
  });

  it("starts from the top when the playhead left the region", () => {
    expect(reloadStartAt(90, draft, FILE)).toBe(0);
    expect(reloadStartAt(170, draft, FILE)).toBe(0);
  });
});

describe("gainAt / envelopePoints", () => {
  const faded = points({
    cue_in_ms: 10_000,
    fade_in_ms: 12_000,
    fade_out_ms: 18_000,
    cue_out_ms: 20_000,
  });

  it("matches the backend envelope", () => {
    expect(gainAt(9.9, faded, FILE)).toBe(0);
    expect(gainAt(11, faded, FILE)).toBeCloseTo(0.5);
    expect(gainAt(15, faded, FILE)).toBe(1);
    expect(gainAt(19, faded, FILE)).toBeCloseTo(0.5);
    expect(gainAt(20, faded, FILE)).toBe(0);
  });

  it("lets the fade-out win where the ramps overlap", () => {
    const overlap = { ...faded, fade_in_ms: 19_000 };
    expect(gainAt(19, overlap, FILE)).toBeCloseTo(0.5);
  });

  it("is full volume with no markers", () => {
    expect(gainAt(5, points({}), FILE)).toBe(1);
  });

  it("traces the ramps through the frame", () => {
    const pts = envelopePoints(faded, FILE, { from: 8, to: 22 });
    expect(pts[0]).toEqual({ t: 8, gain: 0 });
    expect(pts.at(-1)).toEqual({ t: 22, gain: 0 });
    const at = (t: number) => pts.find((p) => p.t === t)?.gain;
    expect(at(10)).toBe(0);
    expect(at(12)).toBe(1);
    expect(at(18)).toBe(1);
    expect(pts.map((p) => p.t)).toEqual(
      [...pts.map((p) => p.t)].sort((a, b) => a - b),
    );
  });

  it("drops a jump just before a hard Cue In", () => {
    const hard = points({ cue_in_ms: 10_000, cue_out_ms: 20_000 });
    const pts = envelopePoints(hard, FILE, { from: 8, to: 22 });
    const i = pts.findIndex((p) => p.t === 10);
    expect(pts[i - 1].gain).toBe(0);
    expect(pts[i].gain).toBe(1);
  });
});

describe("stackFlags", () => {
  const frame = { from: 0, to: 100 };

  it("keeps spread flags on one row", () => {
    const rows = stackFlags(
      [
        { id: "a", t: 10 },
        { id: "b", t: 50 },
      ],
      frame,
      1000,
    );
    expect([...rows.values()]).toEqual([0, 0]);
  });

  it("stacks flags that would overlap", () => {
    const rows = stackFlags(
      [
        { id: "a", t: 10 },
        { id: "b", t: 11 },
        { id: "c", t: 12 },
        { id: "d", t: 60 },
      ],
      frame,
      1000,
    );
    expect(rows.get("a")).toBe(0);
    expect(rows.get("b")).toBe(1);
    expect(rows.get("c")).toBe(2);
    expect(rows.get("d")).toBe(0);
  });

  it("reuses the emptiest row once every row is taken", () => {
    const flags = ["a", "b", "c", "d"].map((id, i) => ({
      id,
      t: 10 + i * 0.1,
    }));
    const rows = stackFlags(flags, frame, 1000);
    expect(rows.get("d")).toBe(0);
  });
});

describe("preRollTarget", () => {
  const d = points({
    cue_in_ms: 100_000,
    fade_out_ms: 150_000,
    cue_out_ms: 160_000,
  });

  it("plays Cue In raw, so its lead-in is heard", () => {
    expect(preRollTarget("cue_in_ms", d, FILE)).toEqual({
      mode: "raw",
      startAt: 98,
    });
  });

  it("auditions other markers from just before them", () => {
    expect(preRollTarget("fade_out_ms", d, FILE)).toEqual({
      mode: "audition",
      startAt: 48,
    });
    expect(preRollTarget("cue_out_ms", d, FILE)).toEqual({
      mode: "audition",
      startAt: 58,
    });
  });

  it("never starts an audition before its top", () => {
    const fade = { ...d, fade_in_ms: 101_000 };
    expect(preRollTarget("fade_in_ms", fade, FILE)).toEqual({
      mode: "audition",
      startAt: 0,
    });
  });

  it("falls back to raw past the end of what airs", () => {
    const late = { ...d, next_start_ms: 170_000 };
    expect(preRollTarget("next_start_ms", late, FILE)).toEqual({
      mode: "raw",
      startAt: 168,
    });
  });
});
