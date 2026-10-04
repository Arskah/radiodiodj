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
