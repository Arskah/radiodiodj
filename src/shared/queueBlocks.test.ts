import { describe, expect, it } from "vitest";
import type { QueueNode, Track } from "./types";
import {
  blockTrackCount,
  flattenQueue,
  occurrenceAt,
  presentCollapsedIds,
} from "./queueBlocks";

const track: Track = {
  id: 5,
  title: "Song",
  artist: "Singer",
  album: "",
  duration: 100,
  play_count: 0,
};
const queue: QueueNode[] = [
  { kind: "standalone", id: "outside", item: { kind: "track", track } },
  {
    kind: "block",
    id: "show",
    title: "Show",
    savedKind: "prepared_show",
    entries: [
      { id: "cue", item: { kind: "stop" } },
      { id: "inside", item: { kind: "track", track } },
    ],
  },
];

describe("queue block projection", () => {
  it("flattens in playback order without treating headers as entries", () => {
    expect(flattenQueue(queue).map((item) => item.kind)).toEqual([
      "track",
      "stop",
      "track",
    ]);
    expect(
      blockTrackCount(queue[1] as Extract<QueueNode, { kind: "block" }>),
    ).toBe(1);
  });

  it("maps flattened positions to particular occurrences", () => {
    expect([0, 1, 2, 3].map((index) => occurrenceAt(queue, index))).toEqual([
      "outside",
      "cue",
      "inside",
      null,
    ]);
  });

  it("forgets collapse keys only after their block leaves the queue", () => {
    expect(presentCollapsedIds(["show", "gone"], queue)).toEqual(["show"]);
    expect(presentCollapsedIds(["show"], queue.slice(0, 1))).toEqual([]);
  });
});
