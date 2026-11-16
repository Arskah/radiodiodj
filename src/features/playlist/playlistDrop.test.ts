import { describe, expect, it } from "vitest";
import { gapAt, moveTarget, queueDropTarget } from "./playlistDrop";
import type { QueueNode } from "../../shared/types";

// Three 40px rows, 4px apart, the first starting at 100.
const rows = [
  { top: 100, height: 40 },
  { top: 144, height: 40 },
  { top: 188, height: 40 },
];

describe("gapAt", () => {
  it("the upper half of a row points at the gap above it", () => {
    expect(gapAt(100, rows)).toBe(0);
    expect(gapAt(163, rows)).toBe(1);
  });

  it("the lower half points at the gap below it", () => {
    expect(gapAt(120, rows)).toBe(1);
    expect(gapAt(183, rows)).toBe(2);
  });

  it("the padding above the first row is the head", () => {
    expect(gapAt(92, rows)).toBe(0);
  });

  it("the space between two rows is the gap between them", () => {
    expect(gapAt(141, rows)).toBe(1);
    expect(gapAt(186, rows)).toBe(2);
  });

  it("anything under the middle of the last row is the end", () => {
    expect(gapAt(208, rows)).toBe(3);
    expect(gapAt(900, rows)).toBe(3);
  });

  it("an empty list has one gap", () => {
    expect(gapAt(50, [])).toBe(0);
  });
});

describe("moveTarget", () => {
  it("a row dropped into a gap above it lands at that gap", () => {
    expect(moveTarget(3, 1)).toBe(1);
    expect(moveTarget(3, 0)).toBe(0);
  });

  it("a row dropped into a gap below it lands one short of it", () => {
    expect(moveTarget(0, 2)).toBe(1);
  });

  it("a drop under the last row moves it to the end", () => {
    expect(moveTarget(0, 3)).toBe(2);
  });

  it("the gaps on either side of the dragged row leave it in place", () => {
    expect(moveTarget(2, 2)).toBeNull();
    expect(moveTarget(2, 3)).toBeNull();
  });
});

describe("queueDropTarget", () => {
  const queue: QueueNode[] = [
    { kind: "standalone", id: "s", item: { kind: "stop" } },
    {
      kind: "block",
      id: "b",
      title: "Show",
      savedKind: "prepared_show",
      entries: [
        { id: "c1", item: { kind: "stop" } },
        { id: "c2", item: { kind: "stop" } },
      ],
    },
    { kind: "standalone", id: "after", item: { kind: "stop" } },
  ];
  const expanded = [
    { kind: "standalone" as const, id: "s", blockId: null, top: 0, height: 40 },
    { kind: "block" as const, id: "b", blockId: null, top: 40, height: 40 },
    { kind: "child" as const, id: "c1", blockId: "b", top: 80, height: 40 },
    { kind: "child" as const, id: "c2", blockId: "b", top: 120, height: 40 },
    {
      kind: "standalone" as const,
      id: "after",
      blockId: null,
      top: 160,
      height: 40,
    },
  ];

  it("targets top-level gaps and child gaps by occurrence ID", () => {
    expect(queueDropTarget(10, expanded, queue, "leaf")).toEqual({
      scope: "main",
      beforeId: "s",
    });
    expect(queueDropTarget(95, expanded, queue, "leaf")).toEqual({
      scope: "block",
      blockId: "b",
      beforeId: "c1",
    });
    expect(queueDropTarget(145, expanded, queue, "leaf")).toEqual({
      scope: "block",
      blockId: "b",
      beforeId: null,
    });
  });

  it("keeps whole-block drags at the top level", () => {
    expect(queueDropTarget(99, expanded, queue, "block")).toEqual({
      scope: "main",
      beforeId: "b",
    });
    expect(queueDropTarget(140, expanded, queue, "block")).toEqual({
      scope: "main",
      beforeId: "after",
    });
    const spaced = expanded.map((row) =>
      row.id === "c1" ? { ...row, top: 88 } : row,
    );
    expect(queueDropTarget(84, spaced, queue, "block")).toEqual({
      scope: "main",
      beforeId: "after",
    });
  });

  it("uses the full-width gutter outside indented children", () => {
    const indented = expanded.map((row) => ({
      ...row,
      left: row.kind === "child" ? 32 : 0,
    }));
    expect(queueDropTarget(95, indented, queue, "leaf", 8)).toEqual({
      scope: "main",
      beforeId: "b",
    });
    expect(queueDropTarget(145, indented, queue, "leaf", 8)).toEqual({
      scope: "main",
      beforeId: "after",
    });
    expect(queueDropTarget(145, indented, queue, "leaf", 48)).toEqual({
      scope: "block",
      blockId: "b",
      beforeId: null,
    });
  });

  it("appends inside a collapsed header center without expanding it", () => {
    const collapsed = expanded.filter((row) => row.kind !== "child");
    expect(queueDropTarget(60, collapsed, queue, "leaf")).toEqual({
      scope: "block",
      blockId: "b",
      beforeId: null,
    });
    expect(queueDropTarget(42, collapsed, queue, "leaf")).toEqual({
      scope: "main",
      beforeId: "b",
    });
    expect(queueDropTarget(78, collapsed, queue, "leaf")).toEqual({
      scope: "main",
      beforeId: "after",
    });
  });
});
