<script lang="ts">
  /**
   * The cue editor's whole-file strip. A click anywhere seeks. Cue In and Cue
   * Out move by the tabs on the region's edges, and only by those: a press on
   * the curve can never grab a marker.
   *
   * The zoom box is the detail strip's window, and it is also its control: its
   * grips resize it and its middle pans it, which takes framing off the region
   * until _Fit_ hands it back.
   */
  import Waveform, { type WaveformMarker } from "../deck/Waveform.svelte";
  import type { Frame } from "../../shared/cueEditor";
  import type { CueMarker } from "../../shared/cuePoints";
  import { createDrag } from "./cueDrag";

  interface Props {
    peaks: number[] | null;
    fileDuration: number;
    /** Resolved Cue In..Cue Out, or null while neither is set. */
    region: Frame | null;
    markers: WaveformMarker[];
    frame: Frame;
    /** The operator sized the zoom by hand, so it no longer follows the region. */
    manual: boolean;
    playhead: number | null;
    onseek: ((t: number) => void) | null;
    onedgemove: (key: CueMarker, t: number) => void;
    onedgeend: () => void;
    /** A grip was dragged: move that edge of the zoom to `t`. */
    onframeresize: (edge: "from" | "to", t: number) => void;
    /** The box was panned: start the zoom at `t`, keeping its width. */
    onframepan: (t: number) => void;
    /** Double-click: hand framing back to the region. */
    onframefit: () => void;
  }

  const {
    peaks,
    fileDuration,
    region,
    markers,
    frame,
    manual,
    playhead,
    onseek,
    onedgemove,
    onedgeend,
    onframeresize,
    onframepan,
    onframefit,
  }: Props = $props();

  let surface: HTMLDivElement | undefined = $state();

  const pct = (t: number): number =>
    fileDuration > 0 ? Math.min(100, Math.max(0, (t / fileDuration) * 100)) : 0;

  function timeAt(clientX: number): number {
    if (!surface) return 0;
    const rect = surface.getBoundingClientRect();
    const f = Math.min(1, Math.max(0, (clientX - rect.left) / rect.width));
    return f * fileDuration;
  }

  const drag = createDrag({
    timeAt,
    onmove: (id, t) => onedgemove(id as CueMarker, t),
    onend: () => onedgeend(),
  });

  const frameDrag = createDrag({
    timeAt,
    onmove: (id, t) => {
      if (id === "pan") onframepan(t);
      else onframeresize(id as "from" | "to", t);
    },
    onend: () => {},
  });

  function onSurfaceDown(e: PointerEvent): void {
    if (!onseek || e.button !== 0) return;
    onseek(timeAt(e.clientX));
  }
</script>

<div class="cue-overview">
  <div class="cue-overview-tabs">
    {#if region}
      {#each [{ key: "cue_in_ms", t: region.from, label: "Cue In" }, { key: "cue_out_ms", t: region.to, label: "Cue Out" }] as edge (edge.key)}
        <button
          class="cue-edge-tab {edge.key === 'cue_in_ms' ? 'cue-in' : 'cue-out'}"
          style:left="{pct(edge.t)}%"
          title="Drag to move {edge.label}"
          aria-label="Move {edge.label}"
          tabindex="-1"
          onpointerdown={(e) => drag.down(e, edge.key, edge.t)}
          onpointermove={drag.move}
          onpointerup={drag.up}
          onpointercancel={drag.up}
        ></button>
      {/each}
    {/if}
  </div>
  <div
    class="cue-overview-curve"
    class:seekable={onseek != null}
    bind:this={surface}
    role="presentation"
    onpointerdown={onSurfaceDown}
  >
    <Waveform
      {peaks}
      progressPct={playhead == null ? 0 : pct(playhead)}
      {markers}
      id="cue-overview"
    />
    {#if region}
      <div
        class="cue-overview-region"
        style:left="{pct(region.from)}%"
        style:width="{pct(region.to) - pct(region.from)}%"
      ></div>
    {/if}
    <div class="cue-overview-shade" style:width="{pct(frame.from)}%"></div>
    <div class="cue-overview-shade right" style:left="{pct(frame.to)}%"></div>
    <div
      class="cue-overview-window"
      class:manual
      style:left="{pct(frame.from)}%"
      style:width="{Math.max(0.5, pct(frame.to) - pct(frame.from))}%"
      role="presentation"
      ondblclick={onframefit}
    >
      <div
        class="cue-grip start"
        role="presentation"
        title="Drag to widen or narrow the zoom"
        onpointerdown={(e) => frameDrag.down(e, "from", frame.from)}
        onpointermove={frameDrag.move}
        onpointerup={frameDrag.up}
        onpointercancel={frameDrag.up}
      ></div>
      <div
        class="cue-pan"
        role="presentation"
        title="Drag to move the zoom · double-click to fit the region"
        onpointerdown={(e) => frameDrag.down(e, "pan", frame.from)}
        onpointermove={frameDrag.move}
        onpointerup={frameDrag.up}
        onpointercancel={frameDrag.up}
      ></div>
      <div
        class="cue-grip end"
        role="presentation"
        title="Drag to widen or narrow the zoom"
        onpointerdown={(e) => frameDrag.down(e, "to", frame.to)}
        onpointermove={frameDrag.move}
        onpointerup={frameDrag.up}
        onpointercancel={frameDrag.up}
      ></div>
    </div>
  </div>
</div>

<style lang="css">
  .cue-overview {
    display: flex;
    flex-direction: column;
  }

  .cue-overview-tabs {
    position: relative;
    height: 12px;
  }

  .cue-edge-tab {
    position: absolute;
    top: 0;
    width: 12px;
    height: 12px;
    margin-left: -6px;
    padding: 0;
    border: none;
    border-radius: 3px 3px 0 0;
    cursor: ew-resize;
    touch-action: none;
  }

  .cue-edge-tab.cue-in {
    background: var(--cue-in-color);
  }

  .cue-edge-tab.cue-out {
    background: var(--cue-out-color);
  }

  .cue-overview-curve {
    position: relative;
    height: 44px;
    background: var(--surface-container-lowest);
    border: 1px solid
      color-mix(in srgb, var(--outline-variant) 20%, transparent);
    border-radius: var(--r-lg, 4px);
    overflow: hidden;
  }

  .cue-overview-curve.seekable {
    cursor: pointer;
  }

  .cue-overview-region {
    position: absolute;
    top: 0;
    bottom: 0;
    background: color-mix(in srgb, var(--primary) 12%, transparent);
    pointer-events: none;
  }

  /* Lens: everything outside the zoom is dimmed, so the box reads as the lit
     part of the track. Only its grips and middle take pointers. */
  .cue-overview-shade {
    position: absolute;
    top: 0;
    bottom: 0;
    left: 0;
    background: color-mix(in srgb, var(--surface-container) 62%, transparent);
    pointer-events: none;
  }

  .cue-overview-shade.right {
    right: 0;
    left: auto;
    width: auto;
  }

  .cue-overview-window {
    position: absolute;
    top: 0;
    bottom: 0;
    display: flex;
    border-radius: 3px;
    box-shadow: inset 0 0 0 1px
      color-mix(in srgb, var(--secondary) 55%, transparent);
  }

  .cue-overview-window.manual {
    box-shadow: inset 0 0 0 1px var(--secondary);
  }

  .cue-grip {
    position: relative;
    width: 8px;
    flex: 0 0 8px;
    background: color-mix(in srgb, var(--secondary) 30%, transparent);
    cursor: ew-resize;
    touch-action: none;
  }

  /* Grab bar down the middle of each grip. */
  .cue-grip::after {
    content: "";
    position: absolute;
    top: 25%;
    bottom: 25%;
    left: 3px;
    width: 2px;
    border-radius: 1px;
    background: color-mix(in srgb, var(--secondary) 85%, transparent);
  }

  .cue-grip:hover {
    background: color-mix(in srgb, var(--secondary) 55%, transparent);
  }

  .cue-grip.start {
    border-radius: 3px 0 0 3px;
  }

  .cue-grip.end {
    border-radius: 0 3px 3px 0;
  }

  .cue-pan {
    flex: 1;
    cursor: grab;
    touch-action: none;
  }

  .cue-pan:active {
    cursor: grabbing;
  }
</style>
