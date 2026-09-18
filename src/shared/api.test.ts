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
