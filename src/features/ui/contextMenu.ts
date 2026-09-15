/**
 * Shape and geometry for the right-click context menu (#314).
 *
 * The placement math lives here as a pure function so its edge cases — a menu
 * opened near the right or bottom edge, or one taller than the viewport — are
 * unit-testable without a DOM.
 */

/** Gap kept between the menu and the viewport edge, in px. */
export const MENU_EDGE_GAP = 8;

export interface MenuItem {
  /** Visible label; also the `{#each}` key, so keep it unique per menu. */
  label: string;
  /** Material Symbols ligature drawn ahead of the label. */
  icon: string;
  /** Runs on activation. The menu closes afterwards either way. */
  onselect: () => void;
  /** Draw a divider above this item, splitting it off the group before it. */
  separated?: boolean;
  /** Marks an action that reaches air (#354): tinted, and never placed first. */
  danger?: boolean;
}

export interface Size {
  width: number;
  height: number;
}

export interface Point {
  x: number;
  y: number;
}

/**
 * Place a menu of size `menu` for a click at `at`, inside `viewport`.
 *
 * The preferred anchor is below-right of the cursor. When the menu would
 * overflow an edge it flips to the other side of the cursor, and if it does not
 * fit there either it is clamped — a menu larger than the viewport pins to the
 * edge gap rather than running off-screen.
 */
export function contextMenuPosition(
  at: Point,
  menu: Size,
  viewport: Size,
): Point {
  return {
    x: place(at.x, menu.width, viewport.width),
    y: place(at.y, menu.height, viewport.height),
  };
}

function place(at: number, size: number, limit: number): number {
  const max = limit - size - MENU_EDGE_GAP;
  if (at <= max) return Math.max(MENU_EDGE_GAP, at);
  return Math.max(MENU_EDGE_GAP, Math.min(max, at - size));
}
