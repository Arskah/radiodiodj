/**
 * Parse a number input, ignoring an empty or unparseable value so a mid-edit
 * blank does not wipe the field. The min-clamp is the backend's job on save.
 */
export function numInput(e: Event, apply: (v: number) => void): void {
  // `Number("")` is 0, which is finite: without the blank check a cleared
  // field reads as a deliberate zero on blur. Harmless where 0 clamps toward
  // a floor, not on a range that excludes it — the dBFS levels would clamp
  // to their *ceiling* and trim every later analysis to nothing.
  const raw = (e.currentTarget as HTMLInputElement).value.trim();
  if (raw === "") return;
  const v = Number(raw);
  if (Number.isFinite(v)) apply(v);
}

/**
 * Parse a comma- or space-separated list of positive integers (backoff
 * schedules). A value that parses to nothing usable is ignored.
 */
export function listInput(e: Event, apply: (v: number[]) => void): void {
  const parsed = (e.currentTarget as HTMLInputElement).value
    .split(/[,\s]+/)
    .map((s) => Number(s))
    .filter((n) => Number.isFinite(n) && n > 0);
  if (parsed.length > 0) apply(parsed);
}

/**
 * What a tuning field showed when the operator entered it. A field they clear
 * never reaches the store — see `numInput` — so the `value` binding has nothing
 * to re-write on save and the box would sit empty until the overlay is
 * reopened. This is what goes back into it.
 */
const shownOnFocus = new WeakMap<HTMLInputElement, string>();

/** Record a number field's displayed value as the operator focuses it. */
export function rememberField(e: FocusEvent): void {
  const el = e.target;
  if (el instanceof HTMLInputElement && el.type === "number") {
    shownOnFocus.set(el, el.value);
  }
}

/** Put the remembered value back into a number field left blank. */
export function restoreCleared(e?: Event): void {
  const el = e?.target;
  if (
    el instanceof HTMLInputElement &&
    el.type === "number" &&
    el.value.trim() === ""
  ) {
    el.value = shownOnFocus.get(el) ?? el.value;
  }
}
