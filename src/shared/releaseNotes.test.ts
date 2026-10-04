import { describe, expect, it } from "vitest";
import { plainNotes } from "./releaseNotes";

describe("plainNotes", () => {
  it("flattens a release-please section", () => {
    const notes = [
      "## [0.26.0](https://github.com/o/r/compare/v0.25.1...v0.26.0) (2026-10-10)",
      "",
      "",
      "### Features",
      "",
      "* **ui:** add an About tab ([#560](https://github.com/o/r/issues/560)) ([62056a7](https://github.com/o/r/commit/62056a7e948b)), closes [#12](https://github.com/o/r/issues/12)",
    ].join("\n");

    expect(plainNotes(notes)).toBe(
      [
        "0.26.0 (2026-10-10)",
        "",
        "Features",
        "",
        "• ui: add an About tab (#560), closes #12",
      ].join("\n"),
    );
  });

  it("leaves plain text alone", () => {
    expect(plainNotes("Fixes a crash.")).toBe("Fixes a crash.");
  });
});
