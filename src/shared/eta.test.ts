import { describe, expect, it } from "vitest";
import { etaDeck, upcomingEtas, type EtaInput } from "./eta";
import { NO_CUE_POINTS } from "./cuePoints";
import { stopMarker, trackItem, type Track } from "./types";

/** A 180-second track whose Next Start, if any, is `nextStart` seconds in. */
const track = (nextStart: number | null = null): Track =>
  ({
    id: 1,
    duration: 180,
    cue_points: {
      ...NO_CUE_POINTS,
      next_start_ms: nextStart && nextStart * 1000,
    },
  }) as Track;

const NOW = 1_000_000;

const etas = (input: Partial<EtaInput>): (number | null)[] =>
  upcomingEtas({
    items: [],
    deck: null,
    hold: null,
    autoAdvance: true,
    nowMs: NOW,
    ...input,
  }).map((ms) => (ms === null ? null : (ms - NOW) / 1000));

describe("upcomingEtas", () => {
  it("starts the first item now with nothing on air", () => {
    expect(etas({ items: [trackItem(track()), trackItem(track())] })).toEqual([
      0, 180,
    ]);
  });

  it("counts from the rest of the track on air, handing over at Next Start", () => {
    const deck = etaDeck(track(170), 100, 180);
    expect(
      etas({ deck, items: [trackItem(track(175)), trackItem(track())] }),
    ).toEqual([70, 245]);
  });

  it("plays each track to Cue Out in Manual", () => {
    const deck = etaDeck(track(170), 100, 180);
    expect(
      etas({
        deck,
        autoAdvance: false,
        items: [trackItem(track(175)), trackItem(track())],
      }),
    ).toEqual([80, 260]);
  });

  it("runs a track to Cue Out ahead of a Stop and adds its plan", () => {
    const items = [trackItem(track(170)), stopMarker(60), trackItem(track())];
    expect(etas({ items })).toEqual([0, 180, 240]);
  });

  it("knows nothing past a Stop without a plan", () => {
    const items = [trackItem(track()), stopMarker(), trackItem(track())];
    expect(etas({ items })).toEqual([0, 180, null]);
  });

  it("resumes at the end of a planned hold, or now once it overruns", () => {
    const items = [trackItem(track())];
    const hold = (startedAgo: number) => ({
      startedAtMs: NOW - startedAgo * 1000,
      plannedDurationSeconds: 120,
    });
    expect(etas({ items, hold: hold(20) })).toEqual([100]);
    expect(etas({ items, hold: hold(200) })).toEqual([0]);
  });

  it("resumes now at a hold without a plan", () => {
    const hold = { startedAtMs: NOW - 5000, plannedDurationSeconds: null };
    expect(etas({ items: [trackItem(track())], hold })).toEqual([0]);
  });
});
