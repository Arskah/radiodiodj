/** Seek continuously while a primary pointer is held on a surface. */
export function createPointerScrub(onmove: (clientX: number) => void) {
  let pointerId: number | null = null;
  let captured: HTMLElement | null = null;

  function finish(e: PointerEvent, release: boolean): void {
    if (e.pointerId !== pointerId) return;
    const element = captured;
    pointerId = null;
    captured = null;
    if (release && element?.hasPointerCapture?.(e.pointerId)) {
      element.releasePointerCapture(e.pointerId);
    }
  }

  return {
    down(e: PointerEvent, element: HTMLElement | undefined): void {
      if (e.button !== 0 || !element || pointerId !== null) return;
      pointerId = e.pointerId;
      captured = element;
      element.setPointerCapture(e.pointerId);
      onmove(e.clientX);
    },

    move(e: PointerEvent): void {
      if (e.pointerId === pointerId) onmove(e.clientX);
    },

    up(e: PointerEvent): void {
      finish(e, true);
    },

    cancel(e: PointerEvent): void {
      finish(e, true);
    },

    lost(e: PointerEvent): void {
      finish(e, false);
    },
  };
}
