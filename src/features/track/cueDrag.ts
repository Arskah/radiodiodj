/**
 * Pointer dragging for the cue editor's handles. A press only arms a drag:
 * nothing moves until the pointer travels `THRESHOLD_PX`, so a click on a
 * handle selects it without nudging it. The grab offset is kept, so a handle
 * never jumps to the pointer when the drag starts.
 */
export const THRESHOLD_PX = 3;

export interface DragOptions {
  /** File time under a viewport x, clamped to the strip. */
  timeAt: (clientX: number) => number;
  onmove: (id: string, t: number) => void;
  onend: (id: string) => void;
  /** Called on every press, moved or not. */
  onpress?: (id: string) => void;
}

interface Active {
  id: string;
  startX: number;
  offset: number;
  moved: boolean;
}

type PointerLike = Pick<PointerEvent, "clientX" | "pointerId" | "button"> & {
  currentTarget: EventTarget | null;
  preventDefault(): void;
  stopPropagation(): void;
};

export function createDrag(opts: DragOptions) {
  let active: Active | null = null;

  return {
    get dragging(): boolean {
      return active?.moved ?? false;
    },

    down(e: PointerLike, id: string, t: number): void {
      if (e.button !== 0) return;
      e.preventDefault();
      e.stopPropagation();
      (e.currentTarget as Element | null)?.setPointerCapture?.(e.pointerId);
      active = {
        id,
        startX: e.clientX,
        offset: opts.timeAt(e.clientX) - t,
        moved: false,
      };
      opts.onpress?.(id);
    },

    move(e: PointerLike): void {
      if (!active) return;
      e.stopPropagation();
      if (!active.moved && Math.abs(e.clientX - active.startX) < THRESHOLD_PX) {
        return;
      }
      active.moved = true;
      opts.onmove(active.id, opts.timeAt(e.clientX) - active.offset);
    },

    up(e: PointerLike): void {
      if (!active) return;
      e.stopPropagation();
      (e.currentTarget as Element | null)?.releasePointerCapture?.(e.pointerId);
      const { id, moved } = active;
      active = null;
      if (moved) opts.onend(id);
    },
  };
}
