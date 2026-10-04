import { describe, expect, it } from "vitest";
import { gapAt, moveTarget } from "./playlistDrop";

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
