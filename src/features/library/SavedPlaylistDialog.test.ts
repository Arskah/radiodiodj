import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import type { Component } from "svelte";

const { app } = vi.hoisted(() => ({
  app: {
    savedDialog: { kind: "saveAs" } as
      { kind: "saveAs" } | { kind: "addTo"; trackIds: number[] } | null,
    playlist: [] as { kind: "track" | "stop" }[],
    savedPlaylists: [],
    isAdmin: true,
    savePlaylistAs: vi.fn().mockResolvedValue(undefined),
    createSavedPlaylist: vi.fn().mockResolvedValue(undefined),
    addToSavedPlaylist: vi.fn().mockResolvedValue(undefined),
    clearSelection: vi.fn(),
  },
}));

vi.mock("../../shared/state.svelte", () => ({ app }));

import SavedPlaylistDialog from "./SavedPlaylistDialog.svelte";

describe("SavedPlaylistDialog", () => {
  let component: ReturnType<typeof mount>;

  const select = (): HTMLSelectElement =>
    document.querySelector("#saved-kind") as HTMLSelectElement;
  const name = (): HTMLInputElement =>
    document.querySelector("#saved-name") as HTMLInputElement;
  const submit = (): HTMLButtonElement =>
    document.querySelector("#btn-saved-confirm") as HTMLButtonElement;

  beforeEach(() => {
    vi.clearAllMocks();
    app.savedDialog = { kind: "saveAs" };
    app.playlist = [];
    component = mount(SavedPlaylistDialog as Component, {
      target: document.body,
    });
    flushSync();
  });

  afterEach(async () => {
    await unmount(component);
    document.body.replaceChildren();
  });

  it("defaults a track-only queue to Playlist and sends the chosen kind", async () => {
    expect(select().value).toBe("playlist");
    name().value = "Pool";
    name().dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
    submit().click();
    await vi.waitFor(() =>
      expect(app.savePlaylistAs).toHaveBeenCalledWith("Pool", "playlist"),
    );
  });

  it("defaults a queue with a Stop to Prepared show and refuses Playlist", async () => {
    await unmount(component);
    app.playlist = [{ kind: "track" }, { kind: "stop" }];
    component = mount(SavedPlaylistDialog as Component, {
      target: document.body,
    });
    flushSync();
    expect(select().value).toBe("prepared_show");

    name().value = "Friday";
    name().dispatchEvent(new Event("input", { bubbles: true }));
    select().value = "playlist";
    select().dispatchEvent(new Event("change", { bubbles: true }));
    flushSync();
    expect(submit().disabled).toBe(true);
    expect(document.body.textContent).toContain(
      "upcoming Stops need a Prepared show",
    );
    expect(app.savePlaylistAs).not.toHaveBeenCalled();

    select().value = "prepared_show";
    select().dispatchEvent(new Event("change", { bubbles: true }));
    flushSync();
    submit().click();
    await vi.waitFor(() =>
      expect(app.savePlaylistAs).toHaveBeenCalledWith(
        "Friday",
        "prepared_show",
      ),
    );
  });

  it("lets a guest create a track-only prepared show from library selection", async () => {
    await unmount(component);
    app.isAdmin = false;
    app.savedDialog = { kind: "addTo", trackIds: [3, 1] };
    component = mount(SavedPlaylistDialog as Component, {
      target: document.body,
    });
    flushSync();
    expect(select().value).toBe("playlist");
    select().value = "prepared_show";
    select().dispatchEvent(new Event("change", { bubbles: true }));
    name().value = "Guest show";
    name().dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
    submit().click();
    await vi.waitFor(() =>
      expect(app.createSavedPlaylist).toHaveBeenCalledWith(
        "Guest show",
        [3, 1],
        "prepared_show",
      ),
    );
  });
});
