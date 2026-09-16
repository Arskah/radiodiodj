/**
 * Renderer-side reading of a track's cue points (#279).
 *
 * The backend owns the rules that *write* cue points: `api.setCuePoints`
 * sorts and bounds the markers and returns what it stored, and the player
 * resolves them again at load time against the decoded duration. Everything
 * here is display-only — it turns already-clamped values into the numbers the
 * UI draws, and never invents or corrects a marker.
 *
 * Deliberately absent: the clamp. One ordering rule in two languages drifts,
 * and the authoritative one runs against a duration the renderer never sees.
 */
import {
  isStopMarker,
  type CuePoints,
  type PlaylistItem,
  type Track,
} from "./types";

/** A track with no adjustments — what "clear all" saves. */
export const NO_CUE_POINTS: CuePoints = {
  cue_in_ms: null,
  fade_in_ms: null,
  fade_out_ms: null,
  cue_out_ms: null,
  next_start_ms: null,
};

/** The five markers, in timeline order. Field name doubles as the marker id. */
export type CueMarker = keyof CuePoints;

/** Colour class for a marker, shared by the waveform lines and the editor. */
export type CueMarkerKind =
  "cue-in" | "fade-in" | "fade-out" | "cue-out" | "next-start";

export interface CueMarkerSpec {
  key: CueMarker;
  kind: CueMarkerKind;
  label: string;
  hint: string;
}

export const CUE_MARKERS: CueMarkerSpec[] = [
  {
    key: "cue_in_ms",
    kind: "cue-in",
    label: "Cue In",
    hint: "Playback starts here",
  },
  {
    key: "fade_in_ms",
    kind: "fade-in",
    label: "Fade In",
    hint: "Full volume reached here",
  },
  {
    key: "fade_out_ms",
    kind: "fade-out",
    label: "Fade Out",
    hint: "Ramp down starts here",
  },
  {
    key: "cue_out_ms",
    kind: "cue-out",
    label: "Cue Out",
    hint: "Playback stops here",
  },
  {
    key: "next_start_ms",
    kind: "next-start",
    label: "Next Start",
    hint: "The next track begins here",
  },
];

/**
 * The set markers of `points`, positioned as fractions of the file — the shape
 * `Waveform.svelte` draws. Unset markers are omitted deliberately: they have no
 * position of their own (each resolves onto a neighbour), and drawing them
 * would stack three grabbable lines on the cue-out.
 */
export function cueMarkerPositions(
  points: CuePoints | undefined | null,
  fileDuration: number,
): { id: CueMarker; kind: CueMarkerKind; label: string; at: number }[] {
  if (!points || fileDuration <= 0) return [];
  return CUE_MARKERS.filter(({ key }) => points[key] != null).map(
    ({ key, kind, label }) => ({
      id: key,
      kind,
      label,
      at: (points[key] as number) / 1000 / fileDuration,
    }),
  );
}

/** Resolved marker positions, in seconds from the start of the file. */
export interface ResolvedCue {
  cueIn: number;
  fadeIn: number;
  fadeOut: number;
  cueOut: number;
  nextStart: number;
}

/** True when any marker is set — i.e. the track has a radio edit. */
export function hasCuePoints(points: CuePoints | undefined | null): boolean {
  if (!points) return false;
  return CUE_MARKERS.some(({ key }) => points[key] != null);
}

/**
 * Apply the documented `null` fallbacks: `cueIn → 0`, `fadeIn → cueIn`,
 * `fadeOut → cueOut`, `cueOut → file end`, `nextStart → cueOut`.
 *
 * `fileDuration` of zero means "unknown", which is also what an untagged file
 * reports; end-anchored markers then resolve to zero and the caller sees a
 * degenerate region rather than a wrong one.
 */
export function resolveCuePoints(
  points: CuePoints | undefined | null,
  fileDuration: number,
): ResolvedCue {
  const end = fileDuration > 0 ? fileDuration : 0;
  const cueIn = secondsOf(points?.cue_in_ms) ?? 0;
  const cueOut = secondsOf(points?.cue_out_ms) ?? end;
  return {
    cueIn,
    fadeIn: secondsOf(points?.fade_in_ms) ?? cueIn,
    fadeOut: secondsOf(points?.fade_out_ms) ?? cueOut,
    cueOut,
    nextStart: secondsOf(points?.next_start_ms) ?? cueOut,
  };
}

/**
 * What the track actually airs, in seconds. Every duration an operator reads —
 * library rows, playlist rows, the decks — is this one, because a column
 * reporting file time the moment a track can be trimmed is a lie.
 */
export function airDuration(track: Track): number {
  const cue = resolveCuePoints(track.cue_points, track.duration);
  return Math.max(0, cue.cueOut - cue.cueIn);
}

/**
 * The track as one airing will actually play it: under `override` when the
 * item carries one, under the track's own radio edit otherwise. Everything
 * that reads cue points off a playlist item — its duration, its trimmed tint —
 * goes through this rather than the raw track.
 */
export function airedTrack(
  track: Track,
  override: CuePoints | undefined | null,
): Track {
  return override ? { ...track, cue_points: override } : track;
}

/**
 * How long `items` keep the station on air unattended, in seconds: the air
 * time of each airing up to the first stop marker. The queue below a stop
 * marker does not play by itself, so counting it would overstate the cover.
 */
export function queueAirTime(items: PlaylistItem[]): number {
  let total = 0;
  for (const item of items) {
    if (isStopMarker(item)) break;
    total += airDuration(airedTrack(item.track, item.cue_override));
  }
  return total;
}

/**
 * True when two marker sets say the same thing, an absent set counting as all
 * `null`. The editor asks it to know whether a draft is dirty and whether the
 * cue deck is auditioning that draft; promotion asks it to know whether an
 * override is worth carrying at all, since one identical to the radio edit is
 * better left off — a later correction then still reaches the queued airing.
 */
export function cuePointsEqual(
  a: CuePoints | undefined | null,
  b: CuePoints | undefined | null,
): boolean {
  return CUE_MARKERS.every(
    ({ key }) => (a?.[key] ?? null) === (b?.[key] ?? null),
  );
}

/** True when the track airs shorter than the file, to within a frame or so. */
export function isTrimmed(track: Track): boolean {
  if (!hasCuePoints(track.cue_points)) return false;
  return Math.abs(airDuration(track) - track.duration) > 0.05;
}

/** Milliseconds to seconds, passing `null`/`undefined` straight through. */
function secondsOf(ms: number | null | undefined): number | null {
  return ms == null ? null : ms / 1000;
}
