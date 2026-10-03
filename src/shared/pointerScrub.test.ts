import { describe, expect, it, vi } from "vitest";
import { createPointerScrub } from "./pointerScrub";

function pointer(
  type: string,
  pointerId: number,
  clientX: number,
  button = 0,
): PointerEvent {
  return new PointerEvent(type, { pointerId, clientX, button });
}

function surface() {
  return {
    setPointerCapture: vi.fn(),
    releasePointerCapture: vi.fn(),
    hasPointerCapture: vi.fn(() => true),
  } as unknown as HTMLElement;
}

describe("createPointerScrub", () => {
  it("seeks immediately, captures the primary pointer, and follows its moves", () => {
    const seek = vi.fn();
    const scrub = createPointerScrub(seek);
    const element = surface();

    scrub.down(pointer("pointerdown", 7, 12), element);
    scrub.move(pointer("pointermove", 7, 25));
    scrub.move(pointer("pointermove", 8, 30));

    expect(seek.mock.calls).toEqual([[12], [25]]);
    expect(element.setPointerCapture).toHaveBeenCalledWith(7);
  });

  it("ignores non-primary presses and releases capture when the matching pointer ends", () => {
    const seek = vi.fn();
    const scrub = createPointerScrub(seek);
    const element = surface();

    scrub.down(pointer("pointerdown", 7, 12, 1), element);
    expect(seek).not.toHaveBeenCalled();
    scrub.down(pointer("pointerdown", 7, 12), element);
    scrub.up(pointer("pointerup", 8, 18));
    expect(element.releasePointerCapture).not.toHaveBeenCalled();
    scrub.up(pointer("pointerup", 7, 20));
    scrub.move(pointer("pointermove", 7, 25));

    expect(seek.mock.calls).toEqual([[12]]);
    expect(element.releasePointerCapture).toHaveBeenCalledWith(7);
  });

  it("ends a scrub on cancel or lost capture", () => {
    for (const finish of ["cancel", "lost"] as const) {
      const seek = vi.fn();
      const scrub = createPointerScrub(seek);
      const element = surface();
      scrub.down(pointer("pointerdown", 7, 12), element);
      scrub[finish](pointer("pointercancel", 7, 18));
      scrub.move(pointer("pointermove", 7, 25));
      expect(seek.mock.calls).toEqual([[12]]);
    }
  });
});
