import { describe, expect, it } from "vitest";
import {
  listInput,
  numInput,
  rememberField,
  restoreCleared,
} from "./numericInput";

function field(value: string, type = "number"): HTMLInputElement {
  const el = document.createElement("input");
  el.type = type;
  el.value = value;
  return el;
}

function event(el: HTMLInputElement): Event {
  const e = new Event("input");
  Object.defineProperty(e, "currentTarget", { value: el });
  Object.defineProperty(e, "target", { value: el });
  return e;
}

describe("numInput", () => {
  it("applies a parsed number", () => {
    let got: number | null = null;
    numInput(event(field("42")), (v) => (got = v));
    expect(got).toBe(42);
  });

  it("applies a negative number, since the dBFS levels are below zero", () => {
    let got: number | null = null;
    numInput(event(field("-48")), (v) => (got = v));
    expect(got).toBe(-48);
  });

  it("ignores a cleared field rather than reading it as a deliberate zero", () => {
    let called = false;
    numInput(event(field("   ")), () => (called = true));
    expect(called).toBe(false);
  });

  it("ignores a value that does not parse", () => {
    let called = false;
    numInput(event(field("abc")), () => (called = true));
    expect(called).toBe(false);
  });
});

describe("listInput", () => {
  // The backoff schedules are text inputs; a number input would refuse the
  // separators outright.
  const list = (value: string): HTMLInputElement => field(value, "text");

  it("parses a comma-separated schedule", () => {
    let got: number[] | null = null;
    listInput(event(list("1000, 2000, 5000")), (v) => (got = v));
    expect(got).toEqual([1000, 2000, 5000]);
  });

  it("parses a space-separated schedule", () => {
    let got: number[] | null = null;
    listInput(event(list("1000 2000")), (v) => (got = v));
    expect(got).toEqual([1000, 2000]);
  });

  it("drops entries that are not positive numbers", () => {
    let got: number[] | null = null;
    listInput(event(list("1000, nope, -5, 0, 2000")), (v) => (got = v));
    expect(got).toEqual([1000, 2000]);
  });

  it("ignores a value with nothing usable in it, keeping the stored schedule", () => {
    let called = false;
    listInput(event(list("nope")), () => (called = true));
    expect(called).toBe(false);
  });
});

describe("restoreCleared", () => {
  it("puts back what the field showed when it was focused", () => {
    const el = field("20");
    rememberField(event(el) as unknown as FocusEvent);
    el.value = "";
    restoreCleared(event(el));
    expect(el.value).toBe("20");
  });

  it("leaves a field the operator actually changed alone", () => {
    const el = field("20");
    rememberField(event(el) as unknown as FocusEvent);
    el.value = "30";
    restoreCleared(event(el));
    expect(el.value).toBe("30");
  });

  it("leaves a blank field alone when nothing was remembered", () => {
    const el = field("");
    restoreCleared(event(el));
    expect(el.value).toBe("");
  });

  it("remembers nothing for a field that is not a number input", () => {
    const el = field("text", "text");
    rememberField(event(el) as unknown as FocusEvent);
    el.value = "";
    restoreCleared(event(el));
    expect(el.value).toBe("");
  });

  it("does nothing without an event, as a programmatic save has none", () => {
    expect(() => restoreCleared()).not.toThrow();
  });
});
