import { describe, expect, it, vi } from "vitest";
import { createDrag } from "./cueDrag";

const ev = (clientX: number, button = 0, pointerId = 1) => ({
  clientX,
  button,
  pointerId,
  currentTarget: null,
  preventDefault: vi.fn(),
  stopPropagation: vi.fn(),
});

// One pixel per second, so the numbers read as both.
const setup = () => {
  const onmove = vi.fn();
  const onend = vi.fn();
  const onpress = vi.fn();
  const drag = createDrag({ timeAt: (x) => x, onmove, onend, onpress });
  return { drag, onmove, onend, onpress };
};

describe("createDrag", () => {
  it("a click selects without moving anything", () => {
    const { drag, onmove, onend, onpress } = setup();
    drag.down(ev(100), "cue_in_ms", 100);
    drag.move(ev(101));
    drag.up(ev(101));
    expect(onpress).toHaveBeenCalledWith("cue_in_ms");
    expect(onmove).not.toHaveBeenCalled();
    expect(onend).not.toHaveBeenCalled();
  });

  it("moves once past the threshold, keeping the grab offset", () => {
    const { drag, onmove, onend } = setup();
    // Grabbed 4 px right of the handle.
    drag.down(ev(104), "cue_out_ms", 100);
    drag.move(ev(110));
    expect(onmove).toHaveBeenLastCalledWith("cue_out_ms", 106);
    expect(drag.dragging).toBe(true);
    drag.up(ev(110));
    expect(onend).toHaveBeenCalledWith("cue_out_ms");
    expect(drag.dragging).toBe(false);
  });

  it("keeps moving once started, even back inside the threshold", () => {
    const { drag, onmove } = setup();
    drag.down(ev(100), "x", 100);
    drag.move(ev(110));
    drag.move(ev(101));
    expect(onmove).toHaveBeenLastCalledWith("x", 101);
  });

  it("ignores anything but the primary button", () => {
    const { drag, onpress, onmove } = setup();
    drag.down(ev(100, 2), "x", 100);
    drag.move(ev(150));
    expect(onpress).not.toHaveBeenCalled();
    expect(onmove).not.toHaveBeenCalled();
  });

  it("never lets a handle press reach the seek surface", () => {
    const { drag } = setup();
    const e = ev(100);
    drag.down(e, "x", 100);
    expect(e.stopPropagation).toHaveBeenCalled();
  });

  it("clears the active drag when pointer capture is lost", () => {
    const { drag, onmove, onend } = setup();
    drag.down(ev(100), "x", 100);
    drag.move(ev(110));
    drag.lost(ev(110));
    drag.move(ev(120));
    drag.up(ev(120));

    expect(onmove).toHaveBeenCalledTimes(1);
    expect(onend).toHaveBeenCalledOnce();
    expect(drag.dragging).toBe(false);
  });

  it("ignores lost capture from a different pointer", () => {
    const { drag, onmove } = setup();
    drag.down(ev(100), "x", 100);
    drag.lost({ ...ev(110), pointerId: 2 });
    drag.move(ev(110));

    expect(onmove).toHaveBeenCalledOnce();
    expect(drag.dragging).toBe(true);
  });

  it("keeps the first pointer when another presses during an active drag", () => {
    const { drag, onmove, onend, onpress } = setup();
    drag.down(ev(100), "first", 100);
    drag.down(ev(200, 0, 2), "second", 200);
    drag.move(ev(110));
    drag.up(ev(110));

    expect(onpress).toHaveBeenCalledOnce();
    expect(onpress).toHaveBeenCalledWith("first");
    expect(onmove).toHaveBeenCalledOnce();
    expect(onmove).toHaveBeenCalledWith("first", 110);
    expect(onend).toHaveBeenCalledOnce();
    expect(onend).toHaveBeenCalledWith("first");
    expect(drag.dragging).toBe(false);
  });
});
