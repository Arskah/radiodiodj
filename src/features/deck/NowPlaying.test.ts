import { afterEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import type { Component } from "svelte";

const { app } = vi.hoisted(() => ({
  app: {
    holdingStop: null as {
      plannedDurationSeconds: number | null;
      elapsedSeconds: number;
      capturedAtUnixMs: number;
      blockId: string | null;
    } | null,
    clockNow: 0,
    currentTrack: null,
    playlist: [],
    autoPlaylistActive: false,
    airTimeRemaining: null,
    autoAdvance: true,
    tailTrack: null,
    tailRemaining: 0,
    isBuffering: false,
    isPlaying: false,
    fading: null,
    fadeMs: 0,
    spinVinyl: true,
    coverArt: null,
    appearance: null,
    progressPct: 0,
    currentTime: 0,
    duration: 0,
    waveform: [],
    airCrop: null,
    prev: vi.fn(),
    togglePlay: vi.fn(),
    stop: vi.fn(),
    next: vi.fn(),
    fadeOut: vi.fn(),
    fadeToNext: vi.fn(),
    toggleMode: vi.fn(),
    seekToPct: vi.fn(),
  },
}));

vi.mock("../../shared/state.svelte", () => ({
  app,
  formatSpan: (seconds: number) => `${seconds}s`,
  formatTime: (seconds: number) => `${seconds}s`,
}));

import NowPlaying from "./NowPlaying.svelte";

describe("NowPlaying held Stop", () => {
  let component: ReturnType<typeof mount>;

  afterEach(async () => {
    if (component) await unmount(component);
    document.body.replaceChildren();
    vi.restoreAllMocks();
  });

  it("shows a read-only hold, retains Play and Next, and hides seek controls", () => {
    app.holdingStop = {
      plannedDurationSeconds: 120,
      elapsedSeconds: 30,
      capturedAtUnixMs: 0,
      blockId: "show",
    };
    component = mount(NowPlaying as Component, { target: document.body });
    flushSync();

    expect(
      document
        .querySelector("#np-title .stop-marker-title")
        ?.getAttribute("aria-label"),
    ).toBe("Stop marker");
    expect(document.querySelector(".deck-status")?.textContent).toBe("Holding");
    expect(document.querySelector(".deck-status")?.classList).not.toContain(
      "holding-overdue",
    );
    expect(document.body.textContent).not.toContain("Holding at stop");
    expect(document.body.textContent).not.toContain("Planned Stop");
    expect(document.querySelector(".deck-remaining")?.textContent).toBe(
      "−1:30",
    );
    expect(document.querySelector("#progress-bar")).toBeNull();
    expect(document.querySelector(".stop-duration-editor")).toBeNull();
    expect(
      (document.querySelector("#btn-play") as HTMLButtonElement).disabled,
    ).toBe(false);
    expect(
      (document.querySelector("#btn-next") as HTMLButtonElement).disabled,
    ).toBe(false);
    expect(
      (document.querySelector("#btn-prev") as HTMLButtonElement).disabled,
    ).toBe(true);
  });

  it("shows a steady over-plan warning after the planned deadline", () => {
    app.holdingStop = {
      plannedDurationSeconds: 2,
      elapsedSeconds: 3,
      capturedAtUnixMs: 0,
      blockId: null,
    };
    component = mount(NowPlaying as Component, { target: document.body });
    flushSync();
    expect(document.querySelector(".deck-remaining")?.textContent).toBe(
      "+0:01",
    );
    expect(document.querySelector(".deck-remaining")?.classList).toContain(
      "stop-over-plan",
    );
    expect(
      document.querySelector(".deck-status.holding-overdue")?.textContent,
    ).toBe("Holding");
    expect(document.body.textContent).not.toContain("Over plan");
  });

  it("shows zero at the deadline and a signed zero counter for fractional overtime", async () => {
    app.holdingStop = {
      plannedDurationSeconds: 2,
      elapsedSeconds: 2,
      capturedAtUnixMs: 0,
      blockId: null,
    };
    component = mount(NowPlaying as Component, { target: document.body });
    flushSync();
    expect(document.querySelector(".deck-remaining")?.textContent).toBe("0:00");
    expect(document.querySelector(".deck-status.holding-overdue")).toBeNull();

    await unmount(component);
    document.body.replaceChildren();
    app.holdingStop = {
      plannedDurationSeconds: 0,
      elapsedSeconds: 0.25,
      capturedAtUnixMs: 0,
      blockId: null,
    };
    component = mount(NowPlaying as Component, { target: document.body });
    flushSync();
    expect(document.querySelector(".deck-remaining")?.textContent).toBe(
      "+0:00",
    );
    expect(
      document.querySelector(".deck-status.holding-overdue")?.textContent,
    ).toBe("Holding");
  });
});
