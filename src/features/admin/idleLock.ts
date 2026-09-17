const ACTIVITY_EVENTS = ["pointerdown", "pointermove", "keydown", "wheel"];

export interface IdleLock {
  /** Restart the countdown, or stop it when `timeoutMs` returns `null`. */
  poke(): void;
  dispose(): void;
}

/**
 * Call `onIdle` once `timeoutMs()` passes without input on `target`. Any
 * input restarts the countdown. `timeoutMs` returning `null` means there is
 * nothing to lock, and no countdown runs.
 */
export function idleLock(
  target: EventTarget,
  timeoutMs: () => number | null,
  onIdle: () => void,
): IdleLock {
  let timer: ReturnType<typeof setTimeout> | undefined;

  function poke(): void {
    clearTimeout(timer);
    timer = undefined;
    const ms = timeoutMs();
    if (ms !== null) timer = setTimeout(onIdle, ms);
  }

  const options = { capture: true, passive: true };
  for (const type of ACTIVITY_EVENTS) {
    target.addEventListener(type, poke, options);
  }

  return {
    poke,
    dispose(): void {
      clearTimeout(timer);
      for (const type of ACTIVITY_EVENTS) {
        target.removeEventListener(type, poke, options);
      }
    },
  };
}
