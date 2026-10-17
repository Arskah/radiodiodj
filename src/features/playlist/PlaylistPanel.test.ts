import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import type { Component } from "svelte";

const { app } = vi.hoisted(() => ({
  app: {
    playlistTab: "playlist",
    playlist: [{ kind: "stop" }],
    upcomingAirTime: 0,
    history: [],
    historyDisplay: [],
    stats: null,
    autoSource: null,
    revertedFrom: null,
    draggedTrackIds: null,
    savedDialog: null as { kind: "saveAs" } | null,
    addStopMarker: vi.fn(),
    removeFromPlaylist: vi.fn(),
    movePlaylistItem: vi.fn(),
    dropDraggedInPlaylist: vi.fn(),
    clearPlaylist: vi.fn(),
    addFiller: vi.fn(),
    clearHover: vi.fn(),
  },
}));

vi.mock("../../shared/state.svelte", () => ({
  app,
  formatSpan: (seconds: number) => `${seconds}s`,
  formatTime: (seconds: number) => `${seconds}s`,
}));

import PlaylistPanel from "./PlaylistPanel.svelte";

describe("PlaylistPanel with a Stop-only queue", () => {
  let component: ReturnType<typeof mount>;

  beforeEach(() => {
    vi.clearAllMocks();
    app.savedDialog = null;
    component = mount(PlaylistPanel as Component, { target: document.body });
    flushSync();
  });

  afterEach(async () => {
    await unmount(component);
    document.body.replaceChildren();
  });

  it("keeps the flat Stop row removable and allows Save as", () => {
    expect(
      document.querySelector(".playlist-row.stop-row")?.textContent,
    ).toContain("Auto play halts here");
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
    expect(app.removeFromPlaylist).toHaveBeenCalledWith(0);
  });
});
