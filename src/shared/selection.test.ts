import { describe, expect, it } from "vitest";
import {
  allSelected,
  clickPicks,
  hiddenCount,
  selectAll,
  selectRange,
  toggle,
  without,
} from "./selection";

const listed = [10, 20, 30, 40, 50];

describe("clickPicks", () => {
  const plain = {
    onCheck: false,
    modifier: false,
    shift: false,
    anyPicked: false,
  };

  it("leaves a plain click on a row alone while nothing is picked", () => {
    expect(clickPicks(plain)).toBe(false);
  });

  it("picks from the check, with Cmd or Ctrl, and with Shift", () => {
    expect(clickPicks({ ...plain, onCheck: true })).toBe(true);
    expect(clickPicks({ ...plain, modifier: true })).toBe(true);
    expect(clickPicks({ ...plain, shift: true })).toBe(true);
  });

  it("picks on a plain click once anything is picked", () => {
    expect(clickPicks({ ...plain, anyPicked: true })).toBe(true);
  });
});

describe("toggle", () => {
  it("picks a row last", () => {
    expect(toggle([30, 10], 20)).toEqual([30, 10, 20]);
  });

  it("drops a row that is already picked", () => {
    expect(toggle([30, 10, 20], 10)).toEqual([30, 20]);
  });

  it("re-picking a row moves it to the end", () => {
    expect(toggle(toggle([10, 20], 10), 10)).toEqual([20, 10]);
  });
});

describe("selectRange", () => {
  it("picks the rows between the anchor and the click in list order", () => {
    expect(selectRange([20], listed, 20, 40)).toEqual([20, 30, 40]);
  });

  it("reads top to bottom when the click is above the anchor", () => {
    expect(selectRange([40], listed, 40, 20)).toEqual([20, 30, 40]);
  });

  it("leaves picks outside the range alone, ahead of the block", () => {
    expect(selectRange([50, 20], listed, 20, 30)).toEqual([50, 20, 30]);
  });

  it("moves a row already picked into the block", () => {
    expect(selectRange([30, 50, 10], listed, 10, 30)).toEqual([50, 10, 20, 30]);
  });

  it("keeps picks that are not in the list", () => {
    expect(selectRange([99, 10], listed, 10, 20)).toEqual([99, 10, 20]);
  });

  it("toggles when there is no anchor", () => {
    expect(selectRange([10], listed, null, 30)).toEqual([10, 30]);
  });

  it("toggles when the anchor is not in the list", () => {
    expect(selectRange([99], listed, 99, 30)).toEqual([99, 30]);
    expect(selectRange([99, 30], listed, 99, 30)).toEqual([99]);
  });
});

describe("selectAll", () => {
  it("picks every listed row in list order", () => {
    expect(selectAll([], listed)).toEqual(listed);
  });

  it("keeps earlier picks where they were and appends the rest", () => {
    expect(selectAll([40, 99, 10], listed)).toEqual([40, 99, 10, 20, 30, 50]);
  });
});

describe("without", () => {
  it("drops the given ids and keeps the order of the rest", () => {
    expect(without([40, 99, 10, 20], listed)).toEqual([99]);
    expect(without([40, 99, 10], [99])).toEqual([40, 10]);
  });
});

describe("allSelected", () => {
  it("is true once every listed row is picked", () => {
    expect(allSelected([50, 40, 30, 20, 10, 99], listed)).toBe(true);
    expect(allSelected([10, 20], listed)).toBe(false);
  });

  it("is false for an empty list", () => {
    expect(allSelected([99], [])).toBe(false);
  });
});

describe("hiddenCount", () => {
  it("counts picks that are not in the list", () => {
    expect(hiddenCount([10, 99, 98], listed)).toBe(2);
    expect(hiddenCount([10, 20], listed)).toBe(0);
  });
});
