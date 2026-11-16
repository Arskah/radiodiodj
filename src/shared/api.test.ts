import { beforeEach, describe, expect, it, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

import { api } from "./api";

beforeEach(() => {
  vi.clearAllMocks();
  invoke.mockResolvedValue({});
});

describe("prepared show commands", () => {
  it("passes the chosen kind when saving the remaining queue", async () => {
    await api.playlistSaveAs("Friday", "prepared_show");
    expect(invoke).toHaveBeenCalledWith("playlist_save_as", {
      name: "Friday",
      kind: "prepared_show",
    });
  });

  it("passes kind when creating a list and keeps track-only creation compatible", async () => {
    await api.savedPlaylistCreate("Friday", [3, 1], "prepared_show");
    await api.savedPlaylistCreate("Pool", [2]);
    expect(invoke.mock.calls[0]).toEqual([
      "saved_playlist_create",
      { name: "Friday", trackIds: [3, 1], kind: "prepared_show" },
    ]);
    expect(invoke.mock.calls[1]).toEqual([
      "saved_playlist_create",
      { name: "Pool", trackIds: [2], kind: "playlist" },
    ]);
  });

  it("adds a Stop at an authored position and changes kind", async () => {
    await api.savedPlaylistAddStop(9, 2);
    await api.savedPlaylistSetKind(9, "playlist");
    expect(invoke.mock.calls).toEqual([
      ["saved_playlist_add_stop", { id: 9, index: 2 }],
      ["saved_playlist_set_kind", { id: 9, kind: "playlist" }],
    ]);
  });
});

describe("playlist block commands", () => {
  it("addresses a whole saved list at a top-level occurrence gap", async () => {
    await api.playlistAddSaved(5, false, "later-block");
    expect(invoke).toHaveBeenCalledWith("playlist_add_saved", {
      id: 5,
      weave: false,
      beforeId: "later-block",
    });
  });

  it("addresses copied and moved children by destination and occurrence ID", async () => {
    const destination = {
      scope: "block" as const,
      blockId: "b",
      beforeId: "child-2",
    };
    await api.playlistCopyTracks([7, 7], destination);
    await api.playlistMoveOccurrence("child-1", destination);
    await api.playlistMoveBlock("b", "later-block");
    expect(invoke.mock.calls).toEqual([
      ["playlist_copy_tracks", { ids: [7, 7], destination }],
      ["playlist_move_occurrence", { id: "child-1", destination }],
      ["playlist_move_block", { blockId: "b", beforeId: "later-block" }],
    ]);
  });

  it("sends Add as next and per-occurrence play, remove, and cue actions", async () => {
    await api.playlistAddAsNext([3, 2]);
    await api.playlistPlayOccurrence("copy-2");
    await api.playlistRemoveOccurrence("copy-2");
    await api.playlistRemoveBlock("b");
    await api.playlistSetOccurrenceCuePoints("copy-1", null);
    expect(invoke.mock.calls).toEqual([
      ["playlist_add_as_next", { ids: [3, 2] }],
      ["playlist_play_occurrence", { id: "copy-2" }],
      ["playlist_remove_occurrence", { id: "copy-2" }],
      ["playlist_remove_block", { blockId: "b" }],
      ["playlist_set_occurrence_cue_points", { id: "copy-1", cuePoints: null }],
    ]);
  });
});

describe("api.updateTrackMetadata", () => {
  it("omits absent keys entirely (leave unchanged)", async () => {
    // Only genre is set — the other fields must not appear in the payload, so
    // the backend leaves them untouched.
    await api.updateTrackMetadata({ id: 1, genre: "Rock" });
    const [cmd, payload] = invoke.mock.calls[0];
    expect(cmd).toBe("update_track_metadata");
    expect(payload).toEqual({ updates: { id: 1, genre: "Rock" } });
    expect(Object.keys(payload.updates).sort()).toEqual(["genre", "id"]);
  });

  it("forwards an empty string as a value to set (not an omission)", async () => {
    // Partial-patch semantics: a present empty string means "set to empty",
    // distinct from omitting the key.
    await api.updateTrackMetadata({ id: 2, album: "" });
    const [, payload] = invoke.mock.calls[0];
    expect(payload.updates).toEqual({ id: 2, album: "" });
    expect(Object.prototype.hasOwnProperty.call(payload.updates, "album")).toBe(
      true,
    );
  });

  it("sends genre/year null through so the backend clears them", async () => {
    // Regression: the wrapper previously dropped `null`, so the advertised
    // "clear a field" feature silently no-oped. A present `null` must reach the
    // backend to distinguish "clear" from "omit".
    await api.updateTrackMetadata({ id: 7, genre: null, year: null });
    const [, payload] = invoke.mock.calls[0];
    expect(payload.updates).toEqual({ id: 7, genre: null, year: null });
    expect(payload.updates.genre).toBeNull();
    expect(payload.updates.year).toBeNull();
  });

  it("forwards concrete title/genre/year values", async () => {
    await api.updateTrackMetadata({
      id: 3,
      title: "New Title",
      artist: "New Artist",
      album: "New Album",
      genre: "Jazz",
      year: 1999,
    });
    const [, payload] = invoke.mock.calls[0];
    expect(payload.updates).toEqual({
      id: 3,
      title: "New Title",
      artist: "New Artist",
      album: "New Album",
      genre: "Jazz",
      year: 1999,
    });
  });
});

describe("api appearance commands", () => {
  it("sends the theme id camelCased, as the command expects", async () => {
    await api.setTheme("station-red");
    expect(invoke.mock.calls[0]).toEqual([
      "set_theme",
      { themeId: "station-red" },
    ]);
  });

  it("sends the vinyl switch as a named flag", async () => {
    await api.setSpinVinyl(false);
    expect(invoke.mock.calls[0]).toEqual([
      "set_spin_vinyl",
      { enabled: false },
    ]);
  });

  it("takes no argument for the readers and the reload", async () => {
    await api.getAppearance();
    await api.listThemes();
    await api.reloadThemes();
    await api.revealThemesDir();

    expect(invoke.mock.calls.map(([cmd]) => cmd)).toEqual([
      "get_appearance",
      "list_themes",
      "reload_themes",
      "reveal_themes_dir",
    ]);
    expect(
      invoke.mock.calls.every(([, payload]) => payload === undefined),
    ).toBe(true);
  });
});

describe("planned Stop commands", () => {
  it("addresses queued copies by occurrence and saved entries by entry id", async () => {
    await api.playlistSetStopDuration("stop-copy", 0);
    await api.savedPlaylistSetStopDuration(7, null);
    expect(invoke.mock.calls).toEqual([
      [
        "playlist_set_stop_duration",
        { occurrenceId: "stop-copy", plannedDurationSeconds: 0 },
      ],
      [
        "saved_playlist_set_stop_duration",
        { entryId: 7, plannedDurationSeconds: null },
      ],
    ]);
  });
});
