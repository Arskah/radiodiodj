import type { QueueDestination, QueueNode } from "../../shared/types";

/**
 * Where a drag over the playlist lands. A **gap** is a position between rows:
 * gap `i` is above row `i`, and gap `length` is under the last row.
 */

/**
 * The gap a pointer at `clientY` points at, given the rows top to bottom: the
 * one above the first row whose middle is still below the pointer. Anything
 * further down, the padding under the last row included, is the end.
 */
export function gapAt(
  clientY: number,
  rows: Pick<DOMRect, "top" | "height">[],
): number {
  const gap = rows.findIndex((row) => clientY < row.top + row.height / 2);
  return gap === -1 ? rows.length : gap;
}

/**
 * The index a row dragged from `from` ends up at when dropped into `gap`, or
 * `null` when the drop would leave it where it is. Taking the row out shifts
 * every gap below it up by one.
 */
export function moveTarget(from: number, gap: number): number | null {
  const to = gap > from ? gap - 1 : gap;
  return to === from ? null : to;
}

/** Geometry of one visible queue row; hidden children have no row. */
export interface QueueDropRow {
  id: string;
  kind: "standalone" | "block" | "child";
  blockId: string | null;
  top: number;
  height: number;
  left?: number;
}

/** Map a pointer to a top-level or child gap, independent of visible indices. */
export function queueDropTarget(
  y: number,
  rows: readonly QueueDropRow[],
  queue: readonly QueueNode[],
  source: "block" | "leaf",
  x = Number.POSITIVE_INFINITY,
): QueueDestination {
  const nextTop = (id: string): string | null => {
    const index = queue.findIndex((node) => node.id === id);
    return queue[index + 1]?.id ?? null;
  };
  const nextChild = (blockId: string, id: string): string | null => {
    const block = queue.find(
      (node) => node.kind === "block" && node.id === blockId,
    );
    if (!block || block.kind !== "block") return null;
    const index = block.entries.findIndex((entry) => entry.id === id);
    return block.entries[index + 1]?.id ?? null;
  };
  for (const row of rows) {
    const bottom = row.top + row.height;
    if (y < row.top || y > bottom) continue;
    const part = (y - row.top) / row.height;
    if (row.kind === "child" && x < (row.left ?? 0)) {
      const siblings = rows.filter(
        (visible) =>
          visible.kind === "child" && visible.blockId === row.blockId,
      );
      const header = rows.find((visible) => visible.id === row.blockId);
      const middle =
        ((header?.top ?? siblings[0].top) +
          siblings[siblings.length - 1].top +
          siblings[siblings.length - 1].height) /
        2;
      return {
        scope: "main",
        beforeId: y < middle ? row.blockId : nextTop(row.blockId!),
      };
    }
    if (row.kind === "standalone" || source === "block") {
      const topId = row.kind === "child" ? row.blockId! : row.id;
      return {
        scope: "main",
        beforeId: part < 0.5 ? topId : nextTop(topId),
      };
    }
    if (row.kind === "child") {
      return {
        scope: "block",
        blockId: row.blockId!,
        beforeId: part < 0.5 ? row.id : nextChild(row.blockId!, row.id),
      };
    }
    const block = queue.find((node) => node.id === row.id);
    if (!block || block.kind !== "block") {
      return { scope: "main", beforeId: row.id };
    }
    const expanded = rows.some(
      (visible) => visible.kind === "child" && visible.blockId === row.id,
    );
    if (part < 0.25) return { scope: "main", beforeId: row.id };
    if (!expanded && part > 0.75) {
      return { scope: "main", beforeId: nextTop(row.id) };
    }
    return {
      scope: "block",
      blockId: row.id,
      beforeId: expanded ? (block.entries[0]?.id ?? null) : null,
    };
  }
  const next = rows.find((row) => y < row.top);
  if (next?.kind === "child") {
    if (source === "block" || x < (next.left ?? 0)) {
      return { scope: "main", beforeId: nextTop(next.blockId!) };
    }
    return { scope: "block", blockId: next.blockId!, beforeId: next.id };
  }
  return { scope: "main", beforeId: next?.id ?? null };
}
