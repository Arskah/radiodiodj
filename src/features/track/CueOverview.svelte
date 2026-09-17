<script lang="ts">
  /**
   * The cue editor's whole-file strip. A click anywhere seeks. Cue In and Cue
   * Out move by the tabs on the region's edges, and only by those: a press on
   * the curve can never grab a marker. The Preview strip's window is outlined
   * so the operator sees where the zoom sits.
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
    playhead: number | null;
    onseek: ((t: number) => void) | null;
    onedgemove: (key: CueMarker, t: number) => void;
    onedgeend: () => void;
  }

  const {
    peaks,
    fileDuration,
    region,
    markers,
    frame,
    playhead,
    onseek,
    onedgemove,
    onedgeend,
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
    <div
      class="cue-overview-window"
      style:left="{pct(frame.from)}%"
      style:width="{Math.max(0.3, pct(frame.to) - pct(frame.from))}%"
    ></div>
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

  .cue-overview-window {
    position: absolute;
    top: 0;
    bottom: 0;
    border: 1px solid color-mix(in srgb, var(--on-surface) 55%, transparent);
    border-radius: 2px;
    pointer-events: none;
  }
</style>
