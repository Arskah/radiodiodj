import type { PlaylistItem, QueueNode } from "./types";

/** Flatten remaining queue contents in playback order, excluding headers. */
export function flattenQueue(queue: readonly QueueNode[]): PlaylistItem[] {
  return queue.flatMap((node) =>
    node.kind === "block"
      ? node.entries.map((entry) => entry.item)
      : [node.item],
  );
}

/** Remaining audio tracks in a block, excluding Stops. */
export function blockTrackCount(
  block: Extract<QueueNode, { kind: "block" }>,
): number {
  return block.entries.filter((entry) => entry.item.kind === "track").length;
}

/** The occurrence at a playback position, counting children but no headers. */
export function occurrenceAt(
  queue: readonly QueueNode[],
  index: number,
): string | null {
  for (const node of queue) {
    if (node.kind === "standalone") {
      if (index === 0) return node.id;
      index--;
      continue;
    }
    if (index < node.entries.length) return node.entries[index].id;
    index -= node.entries.length;
  }
  return null;
}

/** Remove collapse keys for blocks that are no longer queued. */
export function presentCollapsedIds(
  collapsed: readonly string[],
  queue: readonly QueueNode[],
): string[] {
  const present = new Set(queue.map((node) => node.id));
  return collapsed.filter((id) => present.has(id));
}
