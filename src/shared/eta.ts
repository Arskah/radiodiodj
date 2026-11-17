import { airedTrack, resolveCuePoints } from "./cuePoints";
import { isStopMarker, type PlaylistItem, type Track } from "./types";

/** The main deck as the ETAs read it: air seconds, as the deck reports them. */
export interface EtaDeck {
  /** Seconds into the air region. */
  position: number;
  /** Air time of the track, `cueOut − cueIn`. */
  duration: number;
  /** Where the next track begins, in air seconds; the duration without one. */
  nextStart: number;
}

/** A hold at a Stop, as `Hold` carries it. */
export interface EtaHold {
  startedAtMs: number;
  plannedDurationSeconds: number | null;
}

export interface EtaInput {
  items: readonly PlaylistItem[];
  /** The track on the main deck, or `null` with nothing on it. */
  deck: EtaDeck | null;
  hold: EtaHold | null;
  autoAdvance: boolean;
  nowMs: number;
}

/**
 * When each item in `items` is expected to start, in Unix milliseconds, or
 * `null` past a Stop with no plan. A Stop's own ETA is when its hold begins.
 *
 * Each row is the one above plus that one's span: a track's air time, or its
 * Next Start when Auto hands over to a track; a Stop's plan. The first row is
 * counted from now, so a playing deck keeps every ETA still while a paused or
 * empty one lets them all slip together. Manual mode assumes the presenter
 * starts each next item at Cue Out. See `docs/playlist.md#etas`.
 */
export function upcomingEtas({
  items,
  deck,
  hold,
  autoAdvance,
  nowMs,
}: EtaInput): (number | null)[] {
  let at: number | null = nowMs;
  if (hold) {
    if (hold.plannedDurationSeconds !== null) {
      at = Math.max(
        nowMs,
        hold.startedAtMs + hold.plannedDurationSeconds * 1000,
      );
    }
  } else if (deck) {
    const handover =
      autoAdvance && nextIsTrack(items, 0)
        ? Math.min(deck.nextStart, deck.duration)
        : deck.duration;
    at = nowMs + Math.max(0, handover - deck.position) * 1000;
  }

  return items.map((item, i) => {
    const start = at;
    if (at === null) return null;
    if (isStopMarker(item)) {
      const plan = item.planned_duration_seconds ?? null;
      at = plan === null ? null : at + plan * 1000;
    } else {
      const span = trackSpan(
        airedTrack(item.track, item.cue_override),
        autoAdvance && nextIsTrack(items, i + 1),
      );
      at += span * 1000;
    }
    return start;
  });
}

/** The deck's reading of `track`: where Next Start falls in air seconds. */
export function etaDeck(
  track: Track,
  position: number,
  duration: number,
): EtaDeck {
  const cue = resolveCuePoints(track.cue_points, track.duration);
  return { position, duration, nextStart: cue.nextStart - cue.cueIn };
}

/** Seconds from `track` starting to the next item starting. */
function trackSpan(track: Track, handsOver: boolean): number {
  const cue = resolveCuePoints(track.cue_points, track.duration);
  const air = Math.max(0, cue.cueOut - cue.cueIn);
  if (!handsOver) return air;
  return Math.max(0, Math.min(cue.nextStart - cue.cueIn, air));
}

function nextIsTrack(items: readonly PlaylistItem[], from: number): boolean {
  const next = items[from];
  return next !== undefined && !isStopMarker(next);
}
