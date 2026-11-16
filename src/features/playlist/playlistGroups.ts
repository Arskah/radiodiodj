import type { Group, PlaylistItem } from "../../shared/types";

/**
 * The Upcoming list as drawn: the flat playlist with a header over each run of
 * items that share a group. Groups are labels on the items; this is the only
 * place they become structure. See `docs/playlist.md#groups`.
 */
export type UpcomingRow =
  | {
      kind: "header";
      group: Group;
      /** The run's items are `start` up to, not including, `end`. */
      start: number;
      end: number;
      tracks: number;
      stops: number;
      collapsed: boolean;
      /** The track on air is from this group, so the header stays pinned. */
      onAir: boolean;
    }
  | { kind: "item"; index: number; item: PlaylistItem };

export type HeaderRow = Extract<UpcomingRow, { kind: "header" }>;

/** Where a drop lands: ahead of the item at `index`, in `group` or in none. */
export interface DropAt {
  index: number;
  group: Group | null;
}

const groupOf = (item: PlaylistItem | undefined): Group | null =>
  item?.group ?? null;

/**
 * The rows for `items`. The group on air keeps a header at the top even with
 * nothing of it left queued; a collapsed group shows its header alone.
 */
export function upcomingRows(
  items: readonly PlaylistItem[],
  onAir: Group | null,
  collapsed: ReadonlySet<string>,
): UpcomingRow[] {
  const rows: UpcomingRow[] = [];
  if (onAir && groupOf(items[0])?.id !== onAir.id) {
    rows.push({
      kind: "header",
      group: onAir,
      start: 0,
      end: 0,
      tracks: 0,
      stops: 0,
      collapsed: collapsed.has(onAir.id),
      onAir: true,
    });
  }
  let i = 0;
  while (i < items.length) {
    const group = groupOf(items[i]);
    if (!group) {
      rows.push({ kind: "item", index: i, item: items[i] });
      i += 1;
      continue;
    }
    let end = i;
    while (end < items.length && groupOf(items[end])?.id === group.id) end++;
    const run = items.slice(i, end);
    const stops = run.filter((item) => item.kind === "stop").length;
    const header: HeaderRow = {
      kind: "header",
      group,
      start: i,
      end,
      tracks: run.length - stops,
      stops,
      collapsed: collapsed.has(group.id),
      onAir: i === 0 && onAir?.id === group.id,
    };
    rows.push(header);
    if (!header.collapsed) {
      for (let j = i; j < end; j++) {
        rows.push({ kind: "item", index: j, item: items[j] });
      }
    }
    i = end;
  }
  return rows;
}

/**
 * The drop at gap `gap` of the rows drawn, `gap` counted as `gapAt` counts it.
 * A drop between two rows of one group joins it, the gap under an open header
 * included; any other gap is outside every group. The gap under a collapsed
 * header is after the rows it hides.
 */
export function dropAt(rows: readonly UpcomingRow[], gap: number): DropAt {
  const above = rows[gap - 1];
  const below = rows[gap];
  let index: number;
  if (above?.kind === "header" && above.collapsed) index = above.end;
  else if (below?.kind === "item") index = below.index;
  else if (below?.kind === "header") index = below.start;
  else index = above ? itemsEnd(rows) : 0;

  if (above?.kind === "header" && !above.collapsed) {
    return { index, group: above.group };
  }
  const shared =
    above?.kind === "item" &&
    below?.kind === "item" &&
    groupOf(above.item)?.id === groupOf(below.item)?.id;
  return { index, group: shared ? groupOf(above.item) : null };
}

/** A drop on a collapsed header: appended to the group, which stays shut. */
export function dropInto(header: HeaderRow): DropAt {
  return { index: header.end, group: header.group };
}

/**
 * Where a whole group dragged by its header lands: the gap `gap` points at,
 * or, inside another group, ahead of that group.
 */
export function groupGap(rows: readonly UpcomingRow[], gap: number): number {
  const { index, group } = dropAt(rows, gap);
  if (!group) return index;
  const run = rows.find(
    (row): row is HeaderRow =>
      row.kind === "header" &&
      row.group.id === group.id &&
      row.start <= index &&
      index <= row.end,
  );
  return run ? run.start : index;
}

function itemsEnd(rows: readonly UpcomingRow[]): number {
  let end = 0;
  for (const row of rows) {
    end = Math.max(end, row.kind === "item" ? row.index + 1 : row.end);
  }
  return end;
}

/** How many group colours there are; `.group-colour-N` styles each. */
export const GROUP_COLOURS = 4;

const colours = new Map<string, number>();
let nextColour = 0;

/**
 * A group's colour, picked in the order groups are first seen, so a group and
 * the one queued after it differ. Held for as long as the window is open.
 */
export function groupColour(id: string): number {
  let colour = colours.get(id);
  if (colour === undefined) {
    colour = nextColour;
    nextColour = (nextColour + 1) % GROUP_COLOURS;
    colours.set(id, colour);
  }
  return colour;
}
