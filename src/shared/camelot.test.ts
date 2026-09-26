import { describe, expect, it } from "vitest";
import { camelotOf, formatKey } from "./camelot";

/** Every spelling `audio_measure::key::Key::name` can produce. */
const MAJORS = [
  "C",
  "Db",
  "D",
  "Eb",
  "E",
  "F",
  "F#",
  "G",
  "Ab",
  "A",
  "Bb",
  "B",
];
const MINORS = [
  "Cm",
  "C#m",
  "Dm",
  "D#m",
  "Em",
  "Fm",
  "F#m",
  "Gm",
  "G#m",
  "Am",
  "Bbm",
  "Bm",
];

describe("camelotOf", () => {
  it("covers every key the measurement can report", () => {
    for (const key of [...MAJORS, ...MINORS]) {
      expect(camelotOf(key), key).not.toBeNull();
    }
  });

  it("maps each of the twenty-four keys to a distinct code", () => {
    const codes = [...MAJORS, ...MINORS].map(camelotOf);
    expect(new Set(codes).size).toBe(24);
  });

  it("puts a major and its relative minor on the same number", () => {
    // The wheel's whole point: 8B and 8A share seven notes, so they mix.
    expect(camelotOf("C")).toBe("8B");
    expect(camelotOf("Am")).toBe("8A");
    expect(camelotOf("G")).toBe("9B");
    expect(camelotOf("Em")).toBe("9A");
  });

  it("walks the B ring up in fifths", () => {
    const fifths = ["C", "G", "D", "A", "E", "B"];
    const numbers = fifths.map((k) => Number(camelotOf(k)!.replace("B", "")));
    expect(numbers).toEqual([8, 9, 10, 11, 12, 1]);
  });

  it("returns null for a string it does not know", () => {
    // `initial_key` holds whatever a tagger wrote, including a Camelot code.
    expect(camelotOf("8A")).toBeNull();
    expect(camelotOf("H")).toBeNull();
    expect(camelotOf("")).toBeNull();
    expect(camelotOf(null)).toBeNull();
    expect(camelotOf(undefined)).toBeNull();
  });

  it("ignores surrounding whitespace", () => {
    expect(camelotOf(" Am ")).toBe("8A");
  });
});

describe("formatKey", () => {
  it("shows the note name with its Camelot code", () => {
    expect(formatKey("Am")).toBe("Am (8A)");
    expect(formatKey("F#")).toBe("F# (2B)");
  });

  it("shows a name it cannot place on the wheel unadorned", () => {
    expect(formatKey("8A")).toBe("8A");
  });

  it("has nothing to show for an absent key", () => {
    expect(formatKey(null)).toBeNull();
    expect(formatKey(undefined)).toBeNull();
    expect(formatKey("   ")).toBeNull();
  });
});
