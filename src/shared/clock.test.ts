import { describe, expect, it } from "vitest";
import { formatClock, formatEta, msToNextSecond } from "./clock";

describe("formatClock", () => {
  it("pads every part to two digits", () => {
    expect(formatClock(new Date(2026, 10, 16, 9, 5, 7))).toBe("09:05:07");
  });

  it("reads a 24-hour clock through midnight", () => {
    expect(formatClock(new Date(2026, 10, 16, 23, 59, 59))).toBe("23:59:59");
    expect(formatClock(new Date(2026, 10, 17, 0, 0, 0))).toBe("00:00:00");
  });
});

describe("msToNextSecond", () => {
  it("waits for the rest of the current second", () => {
    expect(msToNextSecond(1_000_250)).toBe(750);
    expect(msToNextSecond(1_000_999)).toBe(1);
  });

  it("waits a whole second when called on the boundary", () => {
    expect(msToNextSecond(1_000_000)).toBe(1000);
  });
});

describe("formatEta", () => {
  const at = new Date(2026, 10, 18, 13, 4, 31).getTime();

  it("shows the time to the second", () => {
    expect(formatEta(at)).toBe("13:04:31");
  });

  it("shows an unknown ETA as a dash", () => {
    expect(formatEta(null)).toBe("—");
  });
});
