import { formatTime } from "./time";

/** What the main deck's counter reads during a hold. */
export interface HoldCounter {
  /** `−m:ss` before a plan's end, `+m:ss` after, `m:ss` without a plan. */
  text: string;
  /** Past a planned break's end. Never true without a plan. */
  over: boolean;
}

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
  if (planned === null) return { text: formatTime(seconds), over: false };
  const left = planned - seconds;
  if (left > 0) return { text: `−${formatTime(left)}`, over: false };
  if (left === 0) return { text: "0:00", over: false };
  return { text: `+${formatTime(-left)}`, over: true };
}
