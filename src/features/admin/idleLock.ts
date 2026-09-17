const ACTIVITY_EVENTS = ["pointerdown", "pointermove", "keydown", "wheel"];
// A backgrounded webview may run timers late, so check again when it returns.
const RETURN_EVENTS = ["focus", "visibilitychange"];

export interface IdleLock {
  /** Restart the countdown, or stop it when `timeoutMs` returns `null`. */
  poke(): void;
  dispose(): void;
}

/**
 * Call `onIdle` once `timeoutMs()` passes without input on `target`. Any
 * input restarts the countdown, unless the deadline has already passed: then
 * it locks, since a timer may have fired late or not at all. `timeoutMs`
 * returning `null` means there is nothing to lock, and no countdown runs.
 */
export function idleLock(
  target: EventTarget,
  timeoutMs: () => number | null,
  onIdle: () => void,
  now: () => number = () => Date.now(),
): IdleLock {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let lastActivity = now();

  function arm(): void {
    clearTimeout(timer);
    timer = undefined;
    const ms = timeoutMs();
    if (ms !== null) {
      timer = setTimeout(check, Math.max(0, lastActivity + ms - now()));
    }
  }

  /** Lock if the deadline has passed; otherwise wait for the rest of it. */
  function check(): boolean {
    const ms = timeoutMs();
    if (ms !== null && now() - lastActivity >= ms) {
      clearTimeout(timer);
      timer = undefined;
      onIdle();
      return true;
    }
    arm();
    return false;
  }

  function poke(): void {
    lastActivity = now();
    arm();
  }

  function onActivity(): void {
    if (!check()) poke();
  }

  const options = { capture: true, passive: true };
  for (const type of ACTIVITY_EVENTS) {
    target.addEventListener(type, onActivity, options);
  }
  for (const type of RETURN_EVENTS) {
    target.addEventListener(type, check, options);
  }

  return {
    poke,
    dispose(): void {
      clearTimeout(timer);
      for (const type of ACTIVITY_EVENTS) {
        target.removeEventListener(type, onActivity, options);
      }
      for (const type of RETURN_EVENTS) {
        target.removeEventListener(type, check, options);
      }
    },
  };
}
