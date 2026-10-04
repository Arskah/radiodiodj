/**
 * Where a drag over the playlist lands. A **gap** is a position between rows:
 * gap `i` is above row `i`, and gap `length` is under the last row.
 */

/** The gap a pointer at `clientY` over row `index` points at. */
export function gapAt(
  clientY: number,
  row: Pick<DOMRect, "top" | "height">,
  index: number,
): number {
  return clientY < row.top + row.height / 2 ? index : index + 1;
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
