import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import type { Component } from "svelte";

const { app } = vi.hoisted(() => ({
  app: {
    searchQuery: "",
    savedPlaylists: [
      {
        id: 5,
        name: "Friday",
        kind: "prepared_show",
        entries: 2,
        stops: 1,
        missing: 1,
      },
    ],
    openSaved: {
      id: 5,
      name: "Friday",
      kind: "prepared_show",
      entries: [
        {
          id: 10,
          kind: "track",
          track: null as {
            id: number;
            title: string;
            artist: string;
            album: string;
            duration: number;
            play_count: number;
          } | null,
          artist: "Artist",
          title: "Unmatched",
          duration: 120,
          contentType: "music",
        },
        {
          id: 11,
          kind: "stop",
          track: null,
          artist: "",
          title: "",
          duration: 0,
          contentType: null,
        },
      ],
    },
    autoSource: null,
    cueDevice: null,
    isAdmin: true,
    missingSince: new Map(),
    hiddenAt: new Map(),
    savedNotice: null,
    settingsOpen: false,
    draggedTrackIds: null,
    clearHover: vi.fn(),
    setSavedPlaylistKind: vi.fn().mockResolvedValue(undefined),
    addSavedStop: vi.fn().mockResolvedValue(undefined),
    addSavedToPlaylist: vi.fn().mockResolvedValue(undefined),
    removeSavedEntry: vi.fn(),
    moveSavedEntry: vi.fn(),
    moveSavedEntries: vi.fn(),
    removeSavedEntries: vi.fn(),
    startTrackDrag: vi.fn(),
    startSavedPlaylistDrag: vi.fn(),
    endSavedPlaylistDrag: vi.fn(),
    setAutoSource: vi.fn(),
    exportSavedPlaylist: vi.fn(),
    openSavedPlaylist: vi.fn(),
    closeSavedPlaylist: vi.fn(),
    importSavedPlaylist: vi.fn(),
    savedDialog: null,
  },
}));

vi.mock("../../shared/state.svelte", () => ({
  app,
  formatTime: (seconds: number) => `${seconds}s`,
}));

import SavedPlaylists from "./SavedPlaylists.svelte";

describe("SavedPlaylists prepared-show editor", () => {
  let component: ReturnType<typeof mount>;

  beforeEach(() => {
    vi.clearAllMocks();
    app.openSaved.entries[0].track = null;
    component = mount(SavedPlaylists as Component, { target: document.body });
    flushSync();
  });

  afterEach(async () => {
    await unmount(component);
    document.body.replaceChildren();
  });

  it("shows Stop separately from an unmatched track and has no woven action", () => {
    expect(document.querySelector("#saved-open-size")?.textContent).toContain(
      "1 track · 1 Stop · 1 missing",
    );
    expect(document.querySelector("#btn-saved-weave")).toBeNull();
    const stop = document.querySelector('[data-entry-id="11"]') as HTMLElement;
    expect(stop.classList.contains("unmatched")).toBe(false);
    expect(stop.getAttribute("aria-label")).toBe("Entry 2: Stop");
    expect(stop.querySelector(".missing-badge")).toBeNull();
    expect(stop.querySelector(".track-duration")?.textContent).toBe("");
    expect(
      document.querySelector('[data-entry-id="10"] .missing-badge'),
    ).not.toBeNull();
  });

  it("adds, selects, reorders, and removes a Stop through the stored-order view", () => {
    (document.querySelector("#btn-saved-stop") as HTMLButtonElement).click();
    expect(app.addSavedStop).toHaveBeenCalledOnce();

    const stop = document.querySelector('[data-entry-id="11"]') as HTMLElement;
    (stop.querySelector(".track-no") as HTMLElement).click();
    flushSync();
    expect(stop.getAttribute("aria-pressed")).toBe("true");

    stop.dispatchEvent(new MouseEvent("dragstart", { bubbles: true }));
    const list = document.querySelector("#saved-entries") as HTMLElement;
    list.dispatchEvent(new MouseEvent("drop", { bubbles: true, clientY: -1 }));
    expect(app.moveSavedEntry).toHaveBeenCalledWith(1, 0);

    (stop.querySelector(".btn-saved-remove") as HTMLButtonElement).click();
    expect(app.removeSavedEntry).toHaveBeenCalledWith(11);
  });

  it("sends a kind change and displays the backend-owned kind until refreshed", () => {
    const kind = document.querySelector(
      "#saved-open-kind",
    ) as HTMLSelectElement;
    kind.value = "playlist";
    kind.dispatchEvent(new Event("change", { bubbles: true }));
    expect(app.setSavedPlaylistKind).toHaveBeenCalledWith("playlist");
    expect(kind.value).toBe("prepared_show");
  });

  it("does not silently drop a picked Stop when queuing selected tracks", () => {
    const track = document.querySelector('[data-entry-id="10"]') as HTMLElement;
    const stop = document.querySelector('[data-entry-id="11"]') as HTMLElement;
    (track.querySelector(".track-no") as HTMLElement).click();
    (stop.querySelector(".track-no") as HTMLElement).click();
    flushSync();
    expect(
      (document.querySelector("#btn-add-selection") as HTMLButtonElement)
        .disabled,
    ).toBe(true);
    expect(
      (document.querySelector("#btn-add-selection-next") as HTMLButtonElement)
        .disabled,
    ).toBe(true);
    const drag = new MouseEvent("dragstart", { bubbles: true });
    const transfer = { effectAllowed: "none", setData: vi.fn() };
    Object.defineProperty(drag, "dataTransfer", { value: transfer });
    stop.dispatchEvent(drag);
    expect(app.startTrackDrag).toHaveBeenCalledWith([]);
    expect(transfer.effectAllowed).toBe("move");
  });

  it("still offers a bound track drag as a queue copy", () => {
    app.openSaved.entries[0].track = {
      id: 42,
      title: "Bound",
      artist: "Artist",
      album: "",
      duration: 120,
      play_count: 0,
    };
    const track = document.querySelector('[data-entry-id="10"]') as HTMLElement;
    const drag = new MouseEvent("dragstart", { bubbles: true });
    const transfer = { effectAllowed: "none", setData: vi.fn() };
    Object.defineProperty(drag, "dataTransfer", { value: transfer });
    track.dispatchEvent(drag);
    expect(app.startTrackDrag).toHaveBeenCalledWith([42]);
    expect(transfer.effectAllowed).toBe("copyMove");
  });

  it("drags the open saved-list title as one block, regardless of its entries", () => {
    const title = document.querySelector("#saved-open-name") as HTMLElement;
    const event = new Event("dragstart", { bubbles: true });
    const transfer = { effectAllowed: "none", setData: vi.fn() };
    Object.defineProperty(event, "dataTransfer", { value: transfer });
    title.dispatchEvent(event);
    expect(app.startSavedPlaylistDrag).toHaveBeenCalledWith(5);
    expect(transfer.effectAllowed).toBe("copy");
    title.dispatchEvent(new Event("dragend", { bubbles: true }));
    expect(app.endSavedPlaylistDrag).toHaveBeenCalledOnce();
  });

  it("uses a separate whole-list handle on the saved row", async () => {
    const open = app.openSaved;
    const saved = { ...app.savedPlaylists[0] };
    await unmount(component);
    (app as unknown as { openSaved: null }).openSaved = null;
    app.savedPlaylists[0] = { ...saved, entries: 1, stops: 0 };
    try {
      component = mount(SavedPlaylists as Component, { target: document.body });
      flushSync();
      const handle = document.querySelector(
        ".saved-whole-drag",
      ) as HTMLButtonElement;
      expect(handle).not.toBeNull();
      const event = new Event("dragstart", { bubbles: true });
      const transfer = { effectAllowed: "none", setData: vi.fn() };
      Object.defineProperty(event, "dataTransfer", { value: transfer });
      handle.dispatchEvent(event);
      expect(app.startSavedPlaylistDrag).toHaveBeenCalledWith(5);
      expect(app.startTrackDrag).not.toHaveBeenCalled();
      expect(transfer.effectAllowed).toBe("copy");
    } finally {
      (app as unknown as { openSaved: typeof open }).openSaved = open;
      app.savedPlaylists[0] = saved;
    }
  });
});
