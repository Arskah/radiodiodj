import { describe, expect, it } from "vitest";
import {
  appendMessage,
  findQuery,
  importMessage,
  matchesSearch,
  sortEntries,
  sizeLabel,
} from "./savedPlaylists";

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

describe("importMessage", () => {
  it("names the new saved playlist and what it could not match", () => {
    expect(importMessage("Show (2)", 12, 2)).toBe(
      "Imported “Show (2)”: 12 tracks · 2 missing",
    );
  });
});

describe("matchesSearch", () => {
  const row = ["Here Comes the Sun", "The Beatles", "Abbey Road"];

  it("matches everything when nothing is typed", () => {
    expect(matchesSearch("", row)).toBe(true);
    expect(matchesSearch("  ", row)).toBe(true);
  });

  it("takes each word as the start of a word, across the fields", () => {
    expect(matchesSearch("beat abb", row)).toBe(true);
    expect(matchesSearch("SUN", row)).toBe(true);
    expect(matchesSearch("road beatles here", row)).toBe(true);
  });

  it("does not match inside a word, or a word that is nowhere", () => {
    expect(matchesSearch("eatles", row)).toBe(false);
    expect(matchesSearch("beat stones", row)).toBe(false);
  });

  it("splits on punctuation and keeps letters outside ASCII", () => {
    expect(matchesSearch("yö", ["Hyvää yötä (Live)"])).toBe(true);
    expect(matchesSearch("live", ["Hyvää yötä (Live)"])).toBe(true);
  });
});

describe("sortEntries", () => {
  const row = (position: number, title: string, duration = 100) => ({
    position,
    title,
    artist: "a",
    album: "al",
    plays: 3 - position,
    duration,
  });
  const rows = [row(0, "beta", 300), row(1, "Alpha", 100), row(2, "älä", 200)];
  const order = (sorted: { position: number }[]) =>
    sorted.map((r) => r.position);

  it("keeps the saved playlist's own order by position, and reverses it", () => {
    expect(order(sortEntries(rows, "position", "asc"))).toEqual([0, 1, 2]);
    expect(order(sortEntries(rows, "position", "desc"))).toEqual([2, 1, 0]);
  });

  it("sorts text without regard to case or accents", () => {
    expect(order(sortEntries(rows, "title", "asc"))).toEqual([2, 1, 0]);
    expect(order(sortEntries(rows, "title", "desc"))).toEqual([0, 1, 2]);
  });

  it("sorts durations and play counts as numbers", () => {
    expect(order(sortEntries(rows, "duration", "asc"))).toEqual([1, 2, 0]);
    expect(order(sortEntries(rows, "plays", "asc"))).toEqual([2, 1, 0]);
  });

  it("leaves ties in the saved playlist's order in both directions", () => {
    expect(order(sortEntries(rows, "artist", "asc"))).toEqual([0, 1, 2]);
    expect(order(sortEntries(rows, "artist", "desc"))).toEqual([0, 1, 2]);
  });

  it("does not reorder the rows it was given", () => {
    sortEntries(rows, "title", "asc");
    expect(order(rows)).toEqual([0, 1, 2]);
  });
});

describe("sizeLabel", () => {
  it("mentions missing entries only when there are some", () => {
    expect(sizeLabel(12, 0)).toBe("12 tracks");
    expect(sizeLabel(1, 0)).toBe("1 track");
    expect(sizeLabel(12, 2)).toBe("12 tracks · 2 missing");
  });
});

describe("findQuery", () => {
  const entry = (artist: string, title: string) => ({
    id: 1,
    track: null,
    artist,
    title,
    duration: 100,
    contentType: "music",
  });

  it("is the artist and the title", () => {
    expect(findQuery(entry("Kate Bush", "Cloudbusting"))).toBe(
      "Kate Bush Cloudbusting",
    );
  });

  it("leaves out what is in brackets", () => {
    expect(
      findQuery(
        entry("Kate Bush", "Cloudbusting (Radio Edit) [2018 Remaster]"),
      ),
    ).toBe("Kate Bush Cloudbusting");
  });

  it("copes with an entry that has no text", () => {
    expect(findQuery(entry("", ""))).toBe("");
    expect(findQuery(entry("", "  Ident  "))).toBe("Ident");
  });

  it("reads a bound entry from its track", () => {
    const bound = {
      ...entry("old", "text"),
      track: {
        id: 9,
        title: "Now",
        artist: "Them",
        album: "",
        duration: 1,
        play_count: 0,
      },
    };
    expect(findQuery(bound)).toBe("Them Now");
  });
});
