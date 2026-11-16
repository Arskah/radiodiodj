/** What the main deck's counter reads during a hold. */
export interface HoldCounter {
  text: string;
  /** Past a planned break's end. Never true without a plan. */
  over: boolean;
}

/** Whole seconds as `m:ss`, minutes past an hour included. */
const minutes = (seconds: number): string =>
  `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;

/**
 * The counter `elapsed` seconds into a hold. A planned break counts down from
 * `−m:ss` through `0:00` and on up as `+m:ss`; an unplanned one counts up from
 * `0:00`. Whole seconds, as the clock shows them.
 */
export function holdCounter(
  planned: number | null,
  elapsed: number,
): HoldCounter {
  const seconds = Math.max(0, Math.floor(elapsed));
  if (planned === null) return { text: minutes(seconds), over: false };
  const left = planned - seconds;
  if (left > 0) return { text: `−${minutes(left)}`, over: false };
  if (left === 0) return { text: "0:00", over: false };
  return { text: `+${minutes(-left)}`, over: true };
}
