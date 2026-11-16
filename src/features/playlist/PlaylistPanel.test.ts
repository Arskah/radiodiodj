import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import type { Component } from "svelte";
import type { QueueNode, Track } from "../../shared/types";

const track: Track = {
  id: 7,
  title: "Opening",
  artist: "Presenter",
  album: "",
  duration: 120,
  play_count: 0,
};
const { app } = vi.hoisted(() => ({
  app: {
    playlistTab: "playlist",
    playlist: [] as unknown[],
    queue: [] as QueueNode[],
    currentBlockId: null as string | null,
    collapsedBlockIds: [] as string[],
    upcomingAirTime: 0,
    holdingStop: null as { blockId: string | null } | null,
    history: [],
    historyDisplay: [],
    stats: null,
    autoSource: null,
    revertedFrom: null,
    draggedTrackIds: null as number[] | null,
    draggedSavedPlaylistId: null as number | null,
    savedDialog: null as { kind: "saveAs" } | null,
    missingSince: new Map(),
    hiddenAt: new Map(),
    addStopMarker: vi.fn(),
    removeQueueOccurrence: vi.fn(),
    removeQueueBlock: vi.fn(),
    moveQueueOccurrence: vi.fn(),
    moveQueueBlock: vi.fn(),
    dropDraggedInQueue: vi.fn(),
    toggleBlockCollapsed: vi.fn(),
    playQueueOccurrence: vi.fn(),
    setQueueOccurrenceCuePoints: vi.fn(),
    setStopDuration: vi.fn(),
    upcomingEta: vi.fn((occurrenceId: string): number | null => {
      void occurrenceId;
      return null;
    }),
    clearPlaylist: vi.fn(),
    addFiller: vi.fn(),
    clearHover: vi.fn(),
    setHover: vi.fn(),
  },
}));
vi.mock("../../shared/state.svelte", () => ({
  app,
  formatSpan: (seconds: number) => `${seconds}s`,
  formatTime: (seconds: number) => `${seconds}s`,
}));
vi.mock("../../shared/playlistTiming", () => ({
  formatEta: (value: number | null) => (value === null ? "—" : String(value)),
  formatStopDuration: (value: number | null) =>
    value === null ? "" : String(value),
  parseStopDuration: (value: string) =>
    value.trim() === "2:00" ? 120 : undefined,
}));
import PlaylistPanel from "./PlaylistPanel.svelte";

function drag(target: Element, type: string, y: number, x = 50): void {
  const event = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperties(event, {
    clientY: { value: y },
    clientX: { value: x },
    dataTransfer: {
      value: { effectAllowed: "", dropEffect: "", setData: vi.fn() },
    },
  });
  target.dispatchEvent(event);
  flushSync();
}

describe("PlaylistPanel queue blocks", () => {
  let component: ReturnType<typeof mount>;
  beforeEach(() => {
    vi.clearAllMocks();
    app.playlist = [{ kind: "stop" }];
    app.queue = [{ kind: "standalone", id: "stop-1", item: { kind: "stop" } }];
    app.currentBlockId = null;
    app.collapsedBlockIds = [];
    app.draggedTrackIds = null;
    app.draggedSavedPlaylistId = null;
    app.savedDialog = null;
    app.holdingStop = null;
  });
  afterEach(async () => {
    await unmount(component);
    document.body.replaceChildren();
    vi.restoreAllMocks();
  });
  function render(): void {
    component = mount(PlaylistPanel as Component, { target: document.body });
    flushSync();
  }

  it("keeps a standalone Stop removable and Save as available", () => {
    render();
    expect(document.querySelector(".stop-row")?.textContent).toContain(
      "Auto play halts here",
    );
    const save = document.querySelector(
      "#btn-save-playlist",
    ) as HTMLButtonElement;
    expect(save.disabled).toBe(false);
    save.click();
    expect(app.savedDialog).toEqual({ kind: "saveAs" });
    (
      document.querySelector(
        '.stop-row button[aria-label="Remove stop marker"]',
      ) as HTMLButtonElement
    ).click();
    expect(app.removeQueueOccurrence).toHaveBeenCalledWith("stop-1");
  });

  it("edits a queued Stop copy and shows its projected arrival", () => {
    app.queue = [
      {
        kind: "standalone",
        id: "stop-1",
        item: { kind: "stop", plannedDurationSeconds: null },
      },
    ];
    app.upcomingEta.mockReturnValue(12_345);
    render();
    const input = document.querySelector(
      '.stop-row input[aria-label="Planned Stop duration (m:ss)"]',
    ) as HTMLInputElement;
    input.value = "2:00";
    input.focus();
    input.dispatchEvent(new Event("input", { bubbles: true }));
    input.blur();
    expect(app.setStopDuration).toHaveBeenCalledWith("stop-1", 120);
    expect(document.querySelector(".stop-row .pl-eta")?.textContent).toBe(
      "12345",
    );
    expect(
      document.querySelector(".stop-row .pl-play-action-spacer"),
    ).not.toBeNull();
  });

  it("shows an active named header, indented children, and a labelled disclosure", () => {
    app.queue = [
      {
        kind: "block",
        id: "block-1",
        title: "Morning show",
        savedKind: "prepared_show",
        entries: [
          { id: "track-1", item: { kind: "track", track } },
          { id: "stop-2", item: { kind: "stop" } },
        ],
      },
    ];
    app.playlist = [{ kind: "track", track }, { kind: "stop" }];
    app.currentBlockId = "block-1";
    render();
    const block = document.querySelector(
      '.queue-block[data-queue-id="block-1"]',
    ) as HTMLElement;
    expect(block.textContent).toContain("Morning show");
    expect(
      block
        .querySelector(".queue-block-count")
        ?.textContent?.replace(/\s+/g, " ")
        .trim(),
    ).toBe("1 track");
    expect(block.textContent).toContain("Playing");
    expect(block.draggable).toBe(false);
    expect(document.querySelectorAll(".queue-child")).toHaveLength(2);
    (
      document.querySelector(
        '.queue-child[data-queue-id="stop-2"] .btn-remove',
      ) as HTMLButtonElement
    ).click();
    expect(app.removeQueueOccurrence).toHaveBeenCalledWith("stop-2");
    (
      document.querySelector(
        '.queue-child[data-queue-id="track-1"] .btn-play-track',
      ) as HTMLButtonElement
    ).click();
    expect(app.playQueueOccurrence).toHaveBeenCalledWith("track-1");
    const disclosure = block.querySelector(
      ".queue-disclosure",
    ) as HTMLButtonElement;
    expect(disclosure.getAttribute("aria-expanded")).toBe("true");
    disclosure.click();
    expect(app.toggleBlockCollapsed).toHaveBeenCalledWith("block-1");
    (block.querySelector(".btn-remove") as HTMLButtonElement).click();
    expect(app.removeQueueBlock).toHaveBeenCalledWith("block-1");
  });

  it("keeps an active empty block header visible while its last child is on air", () => {
    app.queue = [
      {
        kind: "block",
        id: "on-air",
        title: "One song",
        savedKind: "playlist",
        entries: [],
      },
    ];
    app.playlist = [];
    app.currentBlockId = "on-air";
    app.collapsedBlockIds = ["on-air"];
    render();
    const block = document.querySelector(".queue-block") as HTMLElement;
    expect(block.textContent).toContain("Playing");
    expect(
      block
        .querySelector(".queue-block-count")
        ?.textContent?.replace(/\s+/g, " ")
        .trim(),
    ).toBe("0 tracks");
    expect(
      block.querySelector(".queue-disclosure")?.getAttribute("aria-expanded"),
    ).toBe("false");
    expect(document.querySelector(".empty")).toBeNull();
  });

  it("routes a copied track inside a collapsed header and a whole list outside", () => {
    app.queue = [
      {
        kind: "block",
        id: "b",
        title: "Show",
        savedKind: "playlist",
        entries: [],
      },
      { kind: "standalone", id: "after", item: { kind: "stop" } },
    ];
    app.playlist = [{ kind: "stop" }];
    app.collapsedBlockIds = ["b"];
    app.draggedTrackIds = [track.id];
    render();
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(
      function (this: HTMLElement) {
        const top = this.dataset.queueId === "b" ? 0 : 40;
        return {
          top,
          left: 0,
          height: 40,
          width: 300,
          bottom: top + 40,
          right: 300,
          x: 0,
          y: top,
          toJSON: vi.fn(),
        };
      },
    );
    const list = document.querySelector("#playlist") as HTMLElement;
    drag(list, "dragover", 20);
    expect(
      document
        .querySelector('.queue-block[data-queue-id="b"]')
        ?.classList.contains("drop-into"),
    ).toBe(true);
    drag(list, "drop", 20);
    expect(app.dropDraggedInQueue).toHaveBeenCalledWith({
      scope: "block",
      blockId: "b",
      beforeId: null,
    });
    app.draggedTrackIds = null;
    app.draggedSavedPlaylistId = 4;
    drag(list, "dragover", 2);
    drag(list, "drop", 2);
    expect(app.dropDraggedInQueue).toHaveBeenCalledWith({
      scope: "main",
      beforeId: "b",
    });
  });

  it("moves a child occurrence out of its block and an unstarted block as a unit", () => {
    app.queue = [
      {
        kind: "block",
        id: "b",
        title: "Show",
        savedKind: "prepared_show",
        entries: [{ id: "child-stop", item: { kind: "stop" } }],
      },
      { kind: "standalone", id: "after", item: { kind: "stop" } },
    ];
    app.playlist = [{ kind: "stop" }, { kind: "stop" }];
    render();
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(
      function (this: HTMLElement) {
        const top =
          this.dataset.queueId === "b"
            ? 0
            : this.dataset.queueId === "child-stop"
              ? 40
              : 80;
        return {
          top,
          left: this.dataset.queueId === "child-stop" ? 32 : 0,
          height: 40,
          width: 300,
          bottom: top + 40,
          right: 300,
          x: 0,
          y: top,
          toJSON: vi.fn(),
        };
      },
    );
    const list = document.querySelector("#playlist") as HTMLElement;
    drag(
      document.querySelector('[data-queue-id="child-stop"]') as HTMLElement,
      "dragstart",
      60,
    );
    drag(list, "drop", 90);
    expect(app.moveQueueOccurrence).toHaveBeenCalledWith("child-stop", {
      scope: "main",
      beforeId: "after",
    });
    drag(
      document.querySelector('[data-queue-id="b"]') as HTMLElement,
      "dragstart",
      20,
    );
    drag(list, "drop", 110);
    expect(app.moveQueueBlock).toHaveBeenCalledWith("b", null);
  });
});
