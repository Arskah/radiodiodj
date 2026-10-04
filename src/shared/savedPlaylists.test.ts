import { describe, expect, it } from "vitest";
import { appendMessage, sizeLabel } from "./savedPlaylists";

describe("appendMessage", () => {
  it("counts what was added", () => {
    expect(appendMessage(31, 0)).toBe("Added 31 tracks");
    expect(appendMessage(1, 0)).toBe("Added 1 track");
  });

  it("names the unmatched entries it left out", () => {
    expect(appendMessage(31, 2)).toBe("Added 31 tracks, skipped 2 unmatched");
  });

  it("says so when there was nothing it could add", () => {
    expect(appendMessage(0, 3)).toBe("Nothing to add: 3 unmatched");
    expect(appendMessage(0, 0)).toBe("Added 0 tracks");
  });
});

describe("sizeLabel", () => {
  it("mentions missing entries only when there are some", () => {
    expect(sizeLabel(12, 0)).toBe("12 tracks");
    expect(sizeLabel(1, 0)).toBe("1 track");
    expect(sizeLabel(12, 2)).toBe("12 tracks · 2 missing");
  });
});
