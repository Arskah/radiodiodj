import { afterEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import type { Component } from "svelte";
import StopDurationEditor from "./StopDurationEditor.svelte";

describe("StopDurationEditor", () => {
  let component: ReturnType<typeof mount>;

  afterEach(async () => {
    if (component) await unmount(component);
    document.body.replaceChildren();
    vi.restoreAllMocks();
  });

  function render(
    plannedDurationSeconds: number | null,
    onSave: (seconds: number | null) => void,
    disabled = false,
  ): HTMLInputElement {
    component = mount(StopDurationEditor as Component, {
      target: document.body,
      props: { plannedDurationSeconds, onSave, disabled },
    });
    flushSync();
    return document.querySelector(".stop-duration-editor") as HTMLInputElement;
  }

  function blur(input: HTMLInputElement): void {
    input.dispatchEvent(new FocusEvent("blur"));
    flushSync();
  }

  it("keeps blank distinct from an explicit zero duration", async () => {
    const save = vi.fn();
    const input = render(null, save);
    expect(input.value).toBe("");
    input.dispatchEvent(new Event("input", { bubbles: true }));
    blur(input);
    expect(save).not.toHaveBeenCalled();

    input.value = "0:00";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    blur(input);
    expect(save).toHaveBeenCalledWith(0);

    await unmount(component);
    const clear = vi.fn();
    const planned = render(0, clear);
    planned.value = "";
    planned.dispatchEvent(new Event("input", { bubbles: true }));
    blur(planned);
    expect(clear).toHaveBeenCalledWith(null);
  });

  it("rejects invalid input and restores the saved value", () => {
    const save = vi.fn();
    const input = render(125, save);
    input.value = "2:75";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    blur(input);
    expect(save).not.toHaveBeenCalled();
    expect(input.value).toBe("2:05");
  });

  it("respects the existing saved-list edit permission", () => {
    const input = render(60, vi.fn(), true);
    expect(input.disabled).toBe(true);
  });
});
