import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { idleLock, type IdleLock } from "./idleLock";

describe("idleLock", () => {
  let target: EventTarget;
  let onIdle: ReturnType<typeof vi.fn<() => void>>;
  let timeout: number | null;
  let lock: IdleLock;

  beforeEach(() => {
    vi.useFakeTimers();
    target = new EventTarget();
    onIdle = vi.fn<() => void>();
    timeout = 1000;
    lock = idleLock(target, () => timeout, onIdle);
  });

  afterEach(() => {
    lock.dispose();
    vi.useRealTimers();
  });

  it("fires once the timeout passes without input", () => {
    lock.poke();
    vi.advanceTimersByTime(999);
    expect(onIdle).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(onIdle).toHaveBeenCalledTimes(1);
  });

  it("restarts the countdown on input", () => {
    lock.poke();
    vi.advanceTimersByTime(800);
    target.dispatchEvent(new Event("keydown"));
    vi.advanceTimersByTime(800);
    expect(onIdle).not.toHaveBeenCalled();
    vi.advanceTimersByTime(200);
    expect(onIdle).toHaveBeenCalledTimes(1);
  });

  it("locks on input after a deadline the timer missed", () => {
    // A backgrounded webview may hold the timer back; the clock moves on.
    lock.poke();
    vi.setSystemTime(Date.now() + 5000);
    target.dispatchEvent(new Event("pointermove"));
    expect(onIdle).toHaveBeenCalledTimes(1);
  });

  it("locks when the window returns after the deadline", () => {
    lock.poke();
    vi.setSystemTime(Date.now() + 5000);
    target.dispatchEvent(new Event("focus"));
    expect(onIdle).toHaveBeenCalledTimes(1);
  });

  it("returning before the deadline does not restart the countdown", () => {
    lock.poke();
    vi.advanceTimersByTime(600);
    target.dispatchEvent(new Event("visibilitychange"));
    vi.advanceTimersByTime(400);
    expect(onIdle).toHaveBeenCalledTimes(1);
  });

  it("uses a changed timeout from the last input", () => {
    lock.poke();
    vi.advanceTimersByTime(500);
    timeout = 600;
    lock.poke();
    vi.advanceTimersByTime(599);
    expect(onIdle).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(onIdle).toHaveBeenCalledTimes(1);
  });

  it("runs no countdown while there is nothing to lock", () => {
    timeout = null;
    lock.poke();
    target.dispatchEvent(new Event("pointerdown"));
    vi.advanceTimersByTime(10_000);
    expect(onIdle).not.toHaveBeenCalled();
  });

  it("stops the countdown when poked with nothing to lock", () => {
    lock.poke();
    timeout = null;
    lock.poke();
    vi.advanceTimersByTime(10_000);
    expect(onIdle).not.toHaveBeenCalled();
  });

  it("ignores input after dispose", () => {
    lock.dispose();
    target.dispatchEvent(new Event("keydown"));
    vi.advanceTimersByTime(10_000);
    expect(onIdle).not.toHaveBeenCalled();
  });
});
