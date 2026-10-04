/**
 * The library selection: track ids in **pick order**, which is the order they
 * are queued in. `listed` is always the ids of the rows in the library list,
 * top to bottom; a selection may hold ids that are not among them.
 */

/** Pick `id` last, or drop it if it is already picked. */
export function toggle(selected: readonly number[], id: number): number[] {
  return selected.includes(id)
    ? selected.filter((s) => s !== id)
    : [...selected, id];
}

/**
 * Pick every row from `anchor` to `id` as one block, in list order whichever
 * end was clicked first, and place the block last. A row of the block that was
 * already picked moves into it. Without an anchor in the list there is no range
 * to speak of, and `id` is toggled.
 */
export function selectRange(
  selected: readonly number[],
  listed: readonly number[],
  anchor: number | null,
  id: number,
): number[] {
  const from = anchor === null ? -1 : listed.indexOf(anchor);
  const to = listed.indexOf(id);
  if (from === -1 || to === -1) return toggle(selected, id);
  const block = listed.slice(Math.min(from, to), Math.max(from, to) + 1);
  return [...without(selected, block), ...block];
}

/** Pick every listed row not picked yet, in list order, after what is. */
export function selectAll(
  selected: readonly number[],
  listed: readonly number[],
): number[] {
  return [...selected, ...without(listed, selected)];
}

/** `selected` less every id in `ids`, order kept. */
export function without(
  selected: readonly number[],
  ids: readonly number[],
): number[] {
  const drop = new Set(ids);
  return selected.filter((id) => !drop.has(id));
}

/** Whether every listed row is picked. An empty list has nothing to pick. */
export function allSelected(
  selected: readonly number[],
  listed: readonly number[],
): boolean {
  if (listed.length === 0) return false;
  const picked = new Set(selected);
  return listed.every((id) => picked.has(id));
}

/** How many picked tracks are not in the list. */
export function hiddenCount(
  selected: readonly number[],
  listed: readonly number[],
): number {
  return without(selected, listed).length;
}
