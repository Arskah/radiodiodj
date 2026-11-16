import { describe, expect, it } from "vitest";
import type { Group, PlaylistItem, Track } from "../../shared/types";
import { stopMarker, trackItem } from "../../shared/types";
import {
  dropAt,
  dropInto,
  GROUP_COLOURS,
  groupColour,
  groupGap,
  upcomingRows,
  type HeaderRow,
  type UpcomingRow,
} from "./playlistGroups";

const show = (id: string): Group => ({ id, name: `Show ${id}` });

const t = (id: number): Track => ({ id }) as Track;

/** `id` a track, `null` a Stop, each in the group named after the slash. */
function queue(...items: [number | null, string?][]): PlaylistItem[] {
  return items.map(([id, group]) => ({
    ...(id === null ? stopMarker() : trackItem(t(id))),
    group: group ? show(group) : null,
  }));
}

/** The rows as short strings: `H:a(0-2)` a header, `3` an item's index. */
function drawn(rows: UpcomingRow[]): string[] {
  return rows.map((row) =>
    row.kind === "header"
      ? `H:${row.group.id}(${row.start}-${row.end})${row.collapsed ? "c" : ""}${row.onAir ? "*" : ""}`
      : String(row.index),
  );
}

const none = new Set<string>();

describe("upcomingRows", () => {
  it("puts a header over each run of one group and none over the rest", () => {
    const items = queue([1], [2, "a"], [null, "a"], [3, "b"], [4]);
    expect(drawn(upcomingRows(items, null, none))).toEqual([
      "0",
      "H:a(1-3)",
      "1",
      "2",
      "H:b(3-4)",
      "3",
      "4",
    ]);
  });

  it("counts a group's tracks and Stops apart", () => {
    const items = queue([1, "a"], [null, "a"], [2, "a"]);
    const header = upcomingRows(items, null, none)[0] as HeaderRow;
    expect([header.tracks, header.stops]).toEqual([2, 1]);
  });

  it("shows a collapsed group as its header alone", () => {
    const items = queue([1, "a"], [2, "a"], [3]);
    expect(drawn(upcomingRows(items, null, new Set(["a"])))).toEqual([
      "H:a(0-2)c",
      "2",
    ]);
  });

  it("pins the group on air, with or without rows of it left", () => {
    const items = queue([1, "a"], [2]);
    expect(drawn(upcomingRows(items, show("a"), none))).toEqual([
      "H:a(0-1)*",
      "0",
      "1",
    ]);
    expect(drawn(upcomingRows(queue([2]), show("a"), none))).toEqual([
      "H:a(0-0)*",
      "0",
    ]);
  });

  it("draws a group split by another row as two runs", () => {
    const items = queue([1, "a"], [2], [3, "a"]);
    expect(drawn(upcomingRows(items, null, none))).toEqual([
      "H:a(0-1)",
      "0",
      "1",
      "H:a(2-3)",
      "2",
    ]);
  });
});

describe("dropAt", () => {
  const items = queue([1], [2, "a"], [3, "a"], [4, "b"]);
  const rows = upcomingRows(items, null, none);
  // 0 | H:a | 1 | 2 | H:b | 3

  it("joins a group between two of its rows, the header included", () => {
    expect(dropAt(rows, 2)).toEqual({ index: 1, group: show("a") });
    expect(dropAt(rows, 3)).toEqual({ index: 2, group: show("a") });
  });

  it("is outside every group at a boundary", () => {
    expect(dropAt(rows, 0)).toEqual({ index: 0, group: null });
    expect(dropAt(rows, 1)).toEqual({ index: 1, group: null });
    expect(dropAt(rows, 4)).toEqual({ index: 3, group: null });
    expect(dropAt(rows, 6)).toEqual({ index: 4, group: null });
  });

  it("lands after the rows a collapsed header hides", () => {
    const shut = upcomingRows(items, null, new Set(["a"]));
    // 0 | H:a(c) | H:b | 3
    expect(dropAt(shut, 2)).toEqual({ index: 3, group: null });
    expect(dropInto(shut[1] as HeaderRow)).toEqual({
      index: 3,
      group: show("a"),
    });
  });

  it("joins the group on air under its pinned header", () => {
    const pinned = upcomingRows(queue([5]), show("a"), none);
    expect(dropAt(pinned, 1)).toEqual({ index: 0, group: show("a") });
    expect(dropAt(pinned, 0)).toEqual({ index: 0, group: null });
  });

  it("lands at the start of an empty playlist", () => {
    expect(dropAt([], 0)).toEqual({ index: 0, group: null });
  });
});

describe("groupGap", () => {
  it("puts a group dropped inside another ahead of that one", () => {
    const rows = upcomingRows(
      queue([1], [2, "a"], [3, "a"], [4, "b"]),
      null,
      none,
    );
    expect(groupGap(rows, 3)).toBe(1);
    expect(groupGap(rows, 4)).toBe(3);
  });
});

describe("groupColour", () => {
  it("keeps a group's colour and gives the next group the next one", () => {
    const first = groupColour("colour-1");
    const second = groupColour("colour-2");
    expect(groupColour("colour-1")).toBe(first);
    expect(second).toBe((first + 1) % GROUP_COLOURS);
  });
});
