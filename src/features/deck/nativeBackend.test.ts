import { beforeEach, describe, expect, it, vi } from "vitest";

const { invoke, listen } = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen }));

import { NativeBackend } from "./nativeBackend";
import type { DeckEvent } from "./backend";

interface ListenCallback {
  (e: { payload: unknown }): void;
}
const listeners: Record<string, ListenCallback> = {};

beforeEach(() => {
  vi.clearAllMocks();
  for (const key of Object.keys(listeners)) delete listeners[key];
  listen.mockImplementation((topic: string, cb: ListenCallback) => {
    listeners[topic] = cb;
    return Promise.resolve(() => {
      delete listeners[topic];
    });
  });
  invoke.mockResolvedValue(undefined);
});

describe("NativeBackend (deckId='main' default)", () => {
  it("subscribes to main-deck:* event topics on construction", async () => {
    const b = new NativeBackend();
    await b.whenReady();
    const topics = listen.mock.calls.map((c) => c[0]);
    expect(topics).toEqual(
      expect.arrayContaining([
        "main-deck:time",
        "main-deck:duration",
        "main-deck:pause-state",
        "main-deck:ended",
        "main-deck:error",
        "main-deck:load-failed",
      ]),
    );
  });

  it("invokes main_deck_* commands", async () => {
    const b = new NativeBackend();
    await b.whenReady();
    await b.play();
    expect(invoke).toHaveBeenCalledWith("main_deck_play");
    await b.pause();
    expect(invoke).toHaveBeenCalledWith("main_deck_pause");
    await b.stop();
    expect(invoke).toHaveBeenCalledWith("main_deck_stop");
    await b.seek(3.5);
    expect(invoke).toHaveBeenCalledWith("main_deck_seek", { seconds: 3.5 });
    await b.setVolume(0.7);
    expect(invoke).toHaveBeenCalledWith("main_deck_set_volume", {
      volume: 0.7,
    });
  });

  it("forwards main-deck:time events to handlers", async () => {
    const b = new NativeBackend();
    const events: DeckEvent[] = [];
    b.on((e) => events.push(e));
    await b.whenReady();
    listeners["main-deck:time"]({ payload: 12.5 });
    listeners["main-deck:duration"]({ payload: 200 });
    listeners["main-deck:pause-state"]({ payload: true });
    listeners["main-deck:ended"]({ payload: null });
    listeners["main-deck:error"]({ payload: "boom" });
    expect(events).toEqual([
      { type: "time", seconds: 12.5 },
      { type: "duration", seconds: 200 },
      { type: "pause-state", paused: true },
      { type: "ended" },
      { type: "error", message: "boom" },
    ]);
  });

  it("forwards main-deck:buffering events to handlers", async () => {
    const b = new NativeBackend();
    const events: DeckEvent[] = [];
    b.on((e) => events.push(e));
    await b.whenReady();
    listeners["main-deck:buffering"]({ payload: true });
    listeners["main-deck:buffering"]({ payload: false });
    expect(events).toEqual([
      { type: "buffering", buffering: true },
      { type: "buffering", buffering: false },
    ]);
  });

  it("forwards main-deck:cache-state events to handlers", async () => {
    const b = new NativeBackend();
    const events: DeckEvent[] = [];
    b.on((e) => events.push(e));
    await b.whenReady();
    listeners["main-deck:cache-state"]({ payload: [1, 2, 3] });
    expect(events).toEqual([{ type: "cache-state", ids: [1, 2, 3] }]);
  });

  it("forwards main-deck:load-failed events with the track id", async () => {
    const b = new NativeBackend();
    const events: DeckEvent[] = [];
    b.on((e) => events.push(e));
    await b.whenReady();
    listeners["main-deck:load-failed"]({ payload: 99 });
    expect(events).toEqual([{ type: "load-failed", id: 99 }]);
  });
});

describe("NativeBackend (deckId='cue')", () => {
  it("subscribes to cue:* event topics", async () => {
    const b = new NativeBackend("cue");
    await b.load(1);
    const topics = listen.mock.calls.map((c) => c[0]);
    expect(topics).toEqual(
      expect.arrayContaining([
        "cue:time",
        "cue:duration",
        "cue:pause-state",
        "cue:ended",
        "cue:error",
        "cue:load-failed",
      ]),
    );
  });

  it("invokes cue_* commands", async () => {
    const b = new NativeBackend("cue");
    await b.load(42);
    expect(invoke).toHaveBeenCalledWith("cue_load", { id: 42 });
    await b.play();
    expect(invoke).toHaveBeenCalledWith("cue_play");
    await b.seek(8);
    expect(invoke).toHaveBeenCalledWith("cue_seek", { seconds: 8 });
    await b.setVolume(0.3);
    expect(invoke).toHaveBeenCalledWith("cue_set_volume", { volume: 0.3 });
  });

  it("forwards cue:* events to handlers", async () => {
    const b = new NativeBackend("cue");
    const events: DeckEvent[] = [];
    b.on((e) => events.push(e));
    await b.load(1);
    listeners["cue:time"]({ payload: 5 });
    expect(events).toEqual([{ type: "time", seconds: 5 }]);
  });

  it("dispose unsubscribes all listeners", async () => {
    const b = new NativeBackend("cue");
    await b.load(1);
    expect(listeners["cue:time"]).toBeDefined();
    await b.dispose();
    expect(listeners["cue:time"]).toBeUndefined();
  });
});
