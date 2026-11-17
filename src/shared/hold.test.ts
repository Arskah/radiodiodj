import { describe, expect, it } from "vitest";
import { holdCounter } from "./hold";

describe("holdCounter", () => {
  it("counts a planned break down to zero and then up past it", () => {
    expect(holdCounter(120, 0)).toEqual({ text: "−2:00", over: false });
    expect(holdCounter(120, 75.9)).toEqual({ text: "−0:45", over: false });
    expect(holdCounter(120, 120.4)).toEqual({ text: "0:00", over: false });
    expect(holdCounter(120, 150)).toEqual({ text: "+0:30", over: true });
  });

  it("counts an unplanned break up and never calls it over", () => {
    expect(holdCounter(null, 0)).toEqual({ text: "0:00", over: false });
    expect(holdCounter(null, 220.7)).toEqual({ text: "3:40", over: false });
  });

  it("takes a zero-length plan as over from its first second", () => {
    expect(holdCounter(0, 0)).toEqual({ text: "0:00", over: false });
    expect(holdCounter(0, 1)).toEqual({ text: "+0:01", over: true });
  });

  it("reads a clock that stepped back as the start", () => {
    expect(holdCounter(60, -3)).toEqual({ text: "−1:00", over: false });
  });
});
