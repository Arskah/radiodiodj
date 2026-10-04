import { describe, expect, it } from "vitest";
import { gapAt, moveTarget } from "./playlistDrop";

const row = { top: 100, height: 40 };

describe("gapAt", () => {
  it("the upper half of a row points at the gap above it", () => {
    expect(gapAt(100, row, 3)).toBe(3);
    expect(gapAt(119, row, 3)).toBe(3);
  });

  it("the lower half points at the gap below it", () => {
    expect(gapAt(120, row, 3)).toBe(4);
    expect(gapAt(139, row, 3)).toBe(4);
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
