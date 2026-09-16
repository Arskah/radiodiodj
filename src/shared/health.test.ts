import { describe, expect, it } from "vitest";
import { checkHasChanges, formatAgo, healthAttention, plural } from "./health";
import type {
  CheckReport,
  DuplicateGroup,
  HealthReport,
  MissingTrack,
} from "./types";

const empty: HealthReport = {
  missing: [],
  missingDismissed: false,
  exact: [],
  possible: [],
  unhashed: 0,
  check: null,
  checkDismissed: false,
  tagWriteFailures: [],
};

const missing = (id: number): MissingTrack => ({
  id,
  title: "t",
  artist: "a",
  path: `/m/${id}`,
  missingSince: 1,
  playCount: 0,
  hasCuePoints: false,
  outsideRoots: false,
});

const group = (key: string, dismissed = false): DuplicateGroup => ({
  key,
  dismissed,
  tracks: [],
});

const check = (extra: Partial<CheckReport> = {}): CheckReport => ({
  checkedAt: 1,
  new: [],
  changed: [],
  gone: [],
  unrooted: [],
  unreachable: [],
  partial: [],
  ...extra,
});

describe("healthAttention", () => {
  it("is zero for a clean library", () => {
    expect(healthAttention(empty)).toBe(0);
    expect(healthAttention({ ...empty, check: check() })).toBe(0);
  });

  it("counts missing tracks once, however many there are", () => {
    const report = { ...empty, missing: [missing(1), missing(2), missing(3)] };
    expect(healthAttention(report)).toBe(1);
    expect(healthAttention({ ...report, missingDismissed: true })).toBe(0);
  });

  it("counts each duplicate group that is not dismissed", () => {
    const report = {
      ...empty,
      exact: [group("a"), group("b", true)],
      possible: [group("c"), group("d")],
    };
    expect(healthAttention(report)).toBe(3);
  });

  it("counts a check with changes once, until it is dismissed", () => {
    const report = {
      ...empty,
      check: check({ new: ["/x", "/y"], gone: ["/z"] }),
    };
    expect(healthAttention(report)).toBe(1);
    expect(healthAttention({ ...report, checkDismissed: true })).toBe(0);
  });

  it("counts every unreachable path, dismissed or not", () => {
    const report = {
      ...empty,
      check: check({ unreachable: ["/a", "/b"] }),
      checkDismissed: true,
    };
    expect(healthAttention(report)).toBe(2);
  });

  it("counts every failed tag write", () => {
    const failure = (id: number) => ({
      id,
      title: "t",
      artist: "a",
      path: `/m/${id}`,
      error: "the file is read-only",
      at: 1,
    });
    expect(
      healthAttention({ ...empty, tagWriteFailures: [failure(1), failure(2)] }),
    ).toBe(2);
  });
});

describe("checkHasChanges", () => {
  it("ignores unreachable and partial paths", () => {
    expect(checkHasChanges(null)).toBe(false);
    expect(
      checkHasChanges(check({ unreachable: ["/a"], partial: ["/b"] })),
    ).toBe(false);
    expect(checkHasChanges(check({ unrooted: ["/c"] }))).toBe(true);
  });
});

describe("formatAgo", () => {
  const now = 10 * 24 * 3_600_000;
  it.each([
    [now - 30_000, "just now"],
    [now - 5 * 60_000, "5 min ago"],
    [now - 3 * 3_600_000, "3 h ago"],
    [now - 2 * 24 * 3_600_000, "2 d ago"],
  ])("%d → %s", (ms, text) => {
    expect(formatAgo(ms, now)).toBe(text);
  });
});

describe("plural", () => {
  it("picks the form by count", () => {
    expect(plural(1, "track")).toBe("1 track");
    expect(plural(0, "track")).toBe("0 tracks");
    expect(plural(2, "copy", "copies")).toBe("2 copies");
  });
});
