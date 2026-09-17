/**
 * Pure geometry and editing rules behind the cue point editor
 * (`CuePointOverlay.svelte`). Everything here is in **file** time — seconds, or
 * milliseconds where a marker value is meant — except where a name says air.
 *
 * The one rule that looks like a clamp is `markerBounds`: operator input stops
 * at the nearest *set* neighbour, so a drag can never produce an order the
 * backend's sort would silently rearrange. The backend clamp stays the
 * authority on what is stored; this only shapes input.
 */
import { NO_CUE_POINTS, resolveCuePoints, type CueMarker } from "./cuePoints";
import type { CuePoints } from "./types";

/** A window onto the file, in seconds. */
export interface Frame {
  from: number;
  to: number;
}

/** Markers whose order is enforced, first to last. Next Start is free. */
export const ORDERED_MARKERS: readonly CueMarker[] = [
  "cue_in_ms",
  "fade_in_ms",
  "fade_out_ms",
  "cue_out_ms",
];

/** Context each side of the region: at least this many seconds… */
export const MARGIN_MIN_S = 5;
/** …or this fraction of the region, whichever is larger. */
export const MARGIN_FRACTION = 0.15;
/** Half-width of the window that follows the playhead when there is no region. */
export const FOLLOW_HALF_S = 30;
/** How far before a marker a pre-roll starts. */
export const PRE_ROLL_S = 2;
/** Nudge steps: plain, Shift, Alt. */
export const NUDGE_MS = { plain: 10, shift: 100, alt: 1000 } as const;

/** Keys that mark a marker at the playhead. */
export const MARK_KEYS: Readonly<Record<string, CueMarker>> = {
  i: "cue_in_ms",
  o: "cue_out_ms",
  f: "fade_in_ms",
  g: "fade_out_ms",
  n: "next_start_ms",
};

export function nudgeStep(mods: {
  shiftKey: boolean;
  altKey: boolean;
}): number {
  if (mods.altKey) return NUDGE_MS.alt;
  if (mods.shiftKey) return NUDGE_MS.shift;
  return NUDGE_MS.plain;
}

/** True once Cue In or Cue Out is set — the Preview strip then frames it. */
export function hasRegion(draft: CuePoints): boolean {
  return draft.cue_in_ms != null || draft.cue_out_ms != null;
}

/** A marker's effective position in ms: its value, or its fallback. */
export function resolvedMs(
  draft: CuePoints,
  key: CueMarker,
  fileDuration: number,
): number {
  const r = resolveCuePoints(draft, fileDuration);
  const seconds: Record<CueMarker, number> = {
    cue_in_ms: r.cueIn,
    fade_in_ms: r.fadeIn,
    fade_out_ms: r.fadeOut,
    cue_out_ms: r.cueOut,
    next_start_ms: r.nextStart,
  };
  return Math.round(seconds[key] * 1000);
}

/** The region, widened by its margin and kept inside the file. */
export function regionFrame(draft: CuePoints, fileDuration: number): Frame {
  const r = resolveCuePoints(draft, fileDuration);
  const margin = Math.max(
    MARGIN_MIN_S,
    Math.max(0, r.cueOut - r.cueIn) * MARGIN_FRACTION,
  );
  return {
    from: Math.max(0, r.cueIn - margin),
    to: Math.min(fileDuration, r.cueOut + margin),
  };
}

/**
 * A fixed-width window around `playhead`, slid rather than cut short at either
 * end of the file.
 */
export function followFrame(playhead: number, fileDuration: number): Frame {
  const width = Math.min(2 * FOLLOW_HALF_S, fileDuration);
  const from = Math.min(
    Math.max(0, playhead - FOLLOW_HALF_S),
    fileDuration - width,
  );
  return { from, to: from + width };
}

/** What the Preview strip shows: the region, or the playhead's neighbourhood. */
export function editorFrame(
  draft: CuePoints,
  fileDuration: number,
  playhead: number | null,
): Frame {
  return hasRegion(draft)
    ? regionFrame(draft, fileDuration)
    : followFrame(playhead ?? 0, fileDuration);
}

/** True when Cue In or Cue Out has moved out of view. */
export function regionOutside(
  frame: Frame,
  draft: CuePoints,
  fileDuration: number,
): boolean {
  const r = resolveCuePoints(draft, fileDuration);
  return r.cueIn < frame.from || r.cueOut > frame.to;
}

/**
 * True when a following window should turn the page: the playhead has left
 * it, or is in its last tenth with more file to come.
 */
export function followNeedsPage(
  frame: Frame,
  playhead: number,
  fileDuration: number,
): boolean {
  if (playhead < frame.from || playhead > frame.to) return true;
  const edge = frame.from + 0.9 * (frame.to - frame.from);
  return playhead > edge && frame.to < fileDuration;
}

/**
 * Where `key` may go, in ms: between its nearest set neighbours in
 * `ORDERED_MARKERS`, and inside the file. Unset neighbours are skipped — an
 * unset Fade In resolves onto Cue In, and counting it would pin Cue In.
 * An unknown duration leaves the top open.
 */
export function markerBounds(
  draft: CuePoints,
  key: CueMarker,
  fileDuration: number,
): { lo: number; hi: number } {
  const end = fileDuration > 0 ? Math.round(fileDuration * 1000) : Infinity;
  const idx = ORDERED_MARKERS.indexOf(key);
  if (idx < 0) return { lo: 0, hi: end };
  const set = (keys: readonly CueMarker[]): number[] =>
    keys.map((k) => draft[k]).filter((v): v is number => v != null);
  const lo = Math.max(0, ...set(ORDERED_MARKERS.slice(0, idx)));
  const hi = Math.min(end, ...set(ORDERED_MARKERS.slice(idx + 1)));
  return { lo, hi: Math.max(lo, hi) };
}

/** `draft` with `key` at `ms`, rounded and kept inside its bounds. */
export function setMarker(
  draft: CuePoints,
  key: CueMarker,
  ms: number,
  fileDuration: number,
): CuePoints {
  const { lo, hi } = markerBounds(draft, key, fileDuration);
  return { ...draft, [key]: Math.min(hi, Math.max(lo, Math.round(ms))) };
}

/** Move `key` by `deltaMs`, starting from its fallback when unset. */
export function nudge(
  draft: CuePoints,
  key: CueMarker,
  deltaMs: number,
  fileDuration: number,
): CuePoints {
  const base = draft[key] ?? resolvedMs(draft, key, fileDuration);
  return setMarker(draft, key, base + deltaMs, fileDuration);
}

export function clearMarker(draft: CuePoints, key: CueMarker): CuePoints {
  return { ...draft, [key]: null };
}

export function emptyDraft(): CuePoints {
  return { ...NO_CUE_POINTS };
}

/**
 * Parse an operator-typed time into ms: `m:ss.mmm`, `ss.mmm`, or `NNNms`.
 * `null` means the text is not a time.
 */
export function parseCueTime(text: string): number | null {
  const t = text.trim();
  let m = /^(\d+)\s*ms$/i.exec(t);
  if (m) return Number(m[1]);
  m = /^(\d+):([0-5]?\d)(?:\.(\d{1,3}))?$/.exec(t);
  if (m) {
    return Number(m[1]) * 60_000 + Number(m[2]) * 1000 + fraction(m[3] ?? "");
  }
  m = /^(\d+)(?:\.(\d{1,3}))?$/.exec(t);
  if (m) return Number(m[1]) * 1000 + fraction(m[2] ?? "");
  return null;
}

function fraction(digits: string): number {
  return digits === "" ? 0 : Number(digits.padEnd(3, "0"));
}

/** `m:ss.mmm`, minutes unbounded. */
export function formatCueTime(ms: number): string {
  const whole = Math.max(0, Math.round(ms));
  const minutes = Math.floor(whole / 60_000);
  const seconds = Math.floor(whole / 1000) % 60;
  const millis = whole % 1000;
  return `${minutes}:${String(seconds).padStart(2, "0")}.${String(millis).padStart(3, "0")}`;
}

/** `m:ss.t`, the transport clock. */
export function formatClock(seconds: number): string {
  const tenths = Math.max(0, Math.round(seconds * 10));
  const minutes = Math.floor(tenths / 600);
  const secs = Math.floor(tenths / 10) % 60;
  return `${minutes}:${String(secs).padStart(2, "0")}.${tenths % 10}`;
}

/**
 * Resample a curve of `step`-second buckets onto `n` buckets spanning
 * `[from, to]`, taking the loudest source bucket each output bucket covers.
 * Anything outside the source reads as silence.
 */
export function rebucket(
  source: ArrayLike<number>,
  step: number,
  from: number,
  to: number,
  n: number,
): number[] {
  const out = new Array<number>(Math.max(0, n)).fill(0);
  if (step <= 0 || to <= from || source.length === 0) return out;
  const width = (to - from) / n;
  for (let i = 0; i < n; i++) {
    const start = from + i * width;
    const first = Math.floor(start / step);
    const last = Math.max(first + 1, Math.ceil((start + width) / step));
    let peak = 0;
    for (let j = Math.max(0, first); j < Math.min(source.length, last); j++) {
      if (source[j] > peak) peak = source[j];
    }
    out[i] = peak;
  }
  return out;
}

/**
 * The deck's position in file time. `applied` is what the deck was loaded
 * with: `null` for raw playback, which already reports file time.
 */
export function playheadFile(
  applied: CuePoints | null,
  fileDuration: number,
  currentTime: number,
): number {
  if (!applied) return currentTime;
  return resolveCuePoints(applied, fileDuration).cueIn + currentTime;
}

/**
 * Where an audition of `draft` should start so it resumes at `playhead`, in
 * air seconds. A playhead before the region starts it from the top; one past
 * the end does too, rather than loading at the very end.
 */
export function reloadStartAt(
  playhead: number,
  draft: CuePoints,
  fileDuration: number,
): number {
  const r = resolveCuePoints(draft, fileDuration);
  const air = playhead - r.cueIn;
  return air > 0 && air < r.cueOut - r.cueIn ? air : 0;
}

/** Port of the backend's `envelope::gain_at`, in file seconds. */
export function gainAt(
  pos: number,
  draft: CuePoints,
  fileDuration: number,
): number {
  const r = resolveCuePoints(draft, fileDuration);
  if (pos < r.cueIn || pos >= r.cueOut) return 0;
  if (r.cueOut > r.fadeOut && pos >= r.fadeOut) {
    return (r.cueOut - pos) / (r.cueOut - r.fadeOut);
  }
  if (r.fadeIn > r.cueIn && pos < r.fadeIn) {
    return (pos - r.cueIn) / (r.fadeIn - r.cueIn);
  }
  return 1;
}

/**
 * The gain envelope over `frame` as polyline vertices. The envelope is linear
 * between its breakpoints, so sampling each breakpoint — and just before it,
 * where it may jump — draws it exactly.
 */
export function envelopePoints(
  draft: CuePoints,
  fileDuration: number,
  frame: Frame,
): { t: number; gain: number }[] {
  const r = resolveCuePoints(draft, fileDuration);
  const EPS = 1e-6;
  const times = new Set<number>([frame.from, frame.to]);
  for (const b of [r.cueIn, r.fadeIn, r.fadeOut, r.cueOut]) {
    if (b > frame.from && b < frame.to) {
      times.add(b - EPS);
      times.add(b);
    }
  }
  return [...times]
    .sort((a, b) => a - b)
    .map((t) => ({ t, gain: gainAt(t, draft, fileDuration) }));
}

/**
 * Give each flag a lane row so neighbours closer than `minGapPx` never
 * overlap. Greedy, left to right; past `rows` rows the least crowded is
 * reused.
 */
export function stackFlags(
  flags: readonly { id: string; t: number }[],
  frame: Frame,
  widthPx: number,
  minGapPx = 56,
  rows = 3,
): Map<string, number> {
  const span = frame.to - frame.from;
  const placed = new Map<string, number>();
  const lastX = new Array<number>(rows).fill(-Infinity);
  const sorted = [...flags].sort((a, b) => a.t - b.t);
  for (const f of sorted) {
    const x = span > 0 ? ((f.t - frame.from) / span) * widthPx : 0;
    let row = lastX.findIndex((l) => x - l >= minGapPx);
    if (row < 0) row = lastX.indexOf(Math.min(...lastX));
    lastX[row] = x;
    placed.set(f.id, row);
  }
  return placed;
}

/**
 * How to pre-roll `key`: Cue In is heard raw, so its lead-in is audible;
 * anything else as an audition starting just before it — unless that is past
 * the end of what airs, where only raw can reach it.
 */
export function preRollTarget(
  key: CueMarker,
  draft: CuePoints,
  fileDuration: number,
): { mode: "raw" | "audition"; startAt: number } {
  const r = resolveCuePoints(draft, fileDuration);
  const at = resolvedMs(draft, key, fileDuration) / 1000 - PRE_ROLL_S;
  if (key === "cue_in_ms" || at >= r.cueOut) {
    return { mode: "raw", startAt: Math.max(0, at) };
  }
  return { mode: "audition", startAt: Math.max(0, at - r.cueIn) };
}
