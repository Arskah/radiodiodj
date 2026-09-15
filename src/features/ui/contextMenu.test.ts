import { describe, it, expect } from "vitest";
import { contextMenuPosition, MENU_EDGE_GAP } from "./contextMenu";

const VIEWPORT = { width: 1000, height: 800 };
const MENU = { width: 200, height: 160 };

describe("contextMenuPosition", () => {
  it("anchors below-right of the cursor when it fits", () => {
    expect(contextMenuPosition({ x: 300, y: 200 }, MENU, VIEWPORT)).toEqual({
      x: 300,
      y: 200,
    });
  });

  it("flips left of the cursor near the right edge", () => {
    const pos = contextMenuPosition({ x: 950, y: 200 }, MENU, VIEWPORT);
    expect(pos.x).toBe(750);
    expect(pos.x + MENU.width).toBeLessThanOrEqual(
      VIEWPORT.width - MENU_EDGE_GAP,
    );
  });

  it("flips above the cursor near the bottom edge", () => {
    const pos = contextMenuPosition({ x: 300, y: 780 }, MENU, VIEWPORT);
    expect(pos.y).toBe(620);
    expect(pos.y + MENU.height).toBeLessThanOrEqual(
      VIEWPORT.height - MENU_EDGE_GAP,
    );
  });

  it("flips both axes in the bottom-right corner and clamps to the gap", () => {
    // Flipping alone still overflows from a cursor this close to the corner,
    // so both axes land on the last position that fits.
    expect(contextMenuPosition({ x: 995, y: 795 }, MENU, VIEWPORT)).toEqual({
      x: VIEWPORT.width - MENU.width - MENU_EDGE_GAP,
      y: VIEWPORT.height - MENU.height - MENU_EDGE_GAP,
    });
  });

  it("keeps the edge gap when the cursor is at the origin", () => {
    expect(contextMenuPosition({ x: 0, y: 0 }, MENU, VIEWPORT)).toEqual({
      x: MENU_EDGE_GAP,
      y: MENU_EDGE_GAP,
    });
  });

  it("pins to the gap when the menu is larger than the viewport", () => {
    const pos = contextMenuPosition(
      { x: 60, y: 40 },
      { width: 400, height: 300 },
      { width: 320, height: 200 },
    );
    expect(pos).toEqual({ x: MENU_EDGE_GAP, y: MENU_EDGE_GAP });
  });
});
