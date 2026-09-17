<script lang="ts" module>
  import type { CueMarker, CueMarkerKind } from "../../shared/cuePoints";

  export interface DetailMarker {
    id: CueMarker;
    kind: CueMarkerKind;
    /** Short flag text. */
    short: string;
    label: string;
    t: number;
  }
</script>

<script lang="ts">
  /**
   * The cue editor's zoomed strip and the handle lane above it. Markers move by
   * their flags in the lane and nowhere else, so a click on the curve is always
   * a seek — even right beside a marker line. The curve is resampled from the
   * fine 10 ms curve when it has arrived, from the stored one until then.
   */
  import {
    formatClock,
    rebucket,
    stackFlags,
    type Frame,
  } from "../../shared/cueEditor";
  import { createDrag } from "./cueDrag";

  interface Props {
    detail: Uint8Array | null;
    /** Seconds per `detail` bucket. */
    detailStep: number;
    peaks: number[] | null;
    fileDuration: number;
    frame: Frame;
    region: Frame | null;
    markers: DetailMarker[];
    envelope: { t: number; gain: number }[] | null;
    playhead: number | null;
    selected: CueMarker | null;
    onseek: ((t: number) => void) | null;
    onselect: (id: CueMarker) => void;
    onmarkermove: (id: CueMarker, t: number) => void;
    onmarkerend: (id: CueMarker) => void;
  }

  const {
    detail,
    detailStep,
    peaks,
    fileDuration,
    frame,
    region,
    markers,
    envelope,
    playhead,
    selected,
    onseek,
    onselect,
    onmarkermove,
    onmarkerend,
  }: Props = $props();

  const BAR_PX = 3;
  const LANE_ROWS = 3;
  const HEIGHT = 100;

  let surface: HTMLDivElement | undefined = $state();
  let width = $state(0);

  $effect(() => {
    if (!surface) return;
    const ro = new ResizeObserver(([entry]) => {
      width = entry.contentRect.width;
    });
    ro.observe(surface);
    return () => ro.disconnect();
  });

  const span = $derived(Math.max(1e-6, frame.to - frame.from));
  const count = $derived(Math.max(1, Math.floor(width / BAR_PX)));

  const bars = $derived.by(() => {
    if (detail && detail.length > 0) {
      return rebucket(detail, detailStep, frame.from, frame.to, count);
    }
    if (peaks && peaks.length > 0 && fileDuration > 0) {
      return rebucket(
        peaks,
        fileDuration / peaks.length,
        frame.from,
        frame.to,
        count,
      );
    }
    return [];
  });

  const pct = (t: number): number => ((t - frame.from) / span) * 100;
  const inView = (t: number): boolean => t >= frame.from && t <= frame.to;

  const rows = $derived(stackFlags(markers, frame, width, 44, LANE_ROWS));

  const clipId = "cue-detail-played";

  const playedPct = $derived(
    playhead == null ? null : Math.min(100, Math.max(0, pct(playhead))),
  );

  const envelopePath = $derived(
    envelope
      ?.map(
        (p) => `${pct(p.t).toFixed(3)},${(HEIGHT - p.gain * 88).toFixed(2)}`,
      )
      .join(" ") ?? "",
  );

  function timeAt(clientX: number): number {
    if (!surface) return frame.from;
    const rect = surface.getBoundingClientRect();
    const f = Math.min(1, Math.max(0, (clientX - rect.left) / rect.width));
    return frame.from + f * span;
  }

  const drag = createDrag({
    timeAt,
    onpress: (id) => onselect(id as CueMarker),
    onmove: (id, t) => onmarkermove(id as CueMarker, t),
    onend: (id) => onmarkerend(id as CueMarker),
  });

  function onSurfaceDown(e: PointerEvent): void {
    if (!onseek || e.button !== 0) return;
    onseek(timeAt(e.clientX));
  }
</script>

<div class="cue-detail">
  <div class="cue-lane" style:height="{LANE_ROWS * 14}px">
    {#each markers as m (m.id)}
      {#if inView(m.t)}
        {@const left = pct(m.t)}
        <button
          class="cue-flag {m.kind}"
          class:selected={selected === m.id}
          class:flip={left > 88}
          style:left="{left}%"
          style:top="{(rows.get(m.id) ?? 0) * 14}px"
          title="{m.label} — drag to move, click to select"
          aria-label="Select {m.label}"
          aria-pressed={selected === m.id}
          onpointerdown={(e) => drag.down(e, m.id, m.t)}
          onpointermove={drag.move}
          onpointerup={drag.up}
          onpointercancel={drag.up}
          onkeydown={(e) => {
            if (e.key === "Enter") onselect(m.id);
          }}>{m.short}</button
        >
      {/if}
    {/each}
  </div>
  <div
    class="cue-detail-curve"
    class:seekable={onseek != null}
    bind:this={surface}
    role="presentation"
    onpointerdown={onSurfaceDown}
  >
    <svg
      class="cue-detail-bars"
      viewBox="0 0 {bars.length || 1} {HEIGHT}"
      preserveAspectRatio="none"
      aria-hidden="true"
    >
      <defs>
        <clipPath id={clipId}>
          <rect
            x="0"
            y="0"
            width={((playedPct ?? 0) / 100) * bars.length}
            height={HEIGHT}
          />
        </clipPath>
      </defs>
      <g class="base">
        {#each bars as v, i (i)}
          {@const h = Math.max(2, (v / 255) * HEIGHT)}
          <rect x={i + 0.15} y={HEIGHT - h} width="0.7" height={h} />
        {/each}
      </g>
      <g class="played" clip-path="url(#{clipId})">
        {#each bars as v, i (i)}
          {@const h = Math.max(2, (v / 255) * HEIGHT)}
          <rect x={i + 0.15} y={HEIGHT - h} width="0.7" height={h} />
        {/each}
      </g>
    </svg>
    {#if region}
      {#if region.from > frame.from}
        <div
          class="cue-detail-dim"
          style:left="0%"
          style:width="{pct(region.from)}%"
        ></div>
      {/if}
      {#if region.to < frame.to}
        <div
          class="cue-detail-dim"
          style:left="{pct(region.to)}%"
          style:right="0"
        ></div>
      {/if}
    {/if}
    {#each markers as m (m.id)}
      {#if inView(m.t)}
        <div
          class="cue-detail-line {m.kind}"
          class:selected={selected === m.id}
          style:left="{pct(m.t)}%"
        ></div>
      {/if}
    {/each}
    {#if envelopePath}
      <svg
        class="cue-detail-envelope"
        viewBox="0 0 100 {HEIGHT}"
        preserveAspectRatio="none"
        aria-hidden="true"
      >
        <polyline points={envelopePath} vector-effect="non-scaling-stroke" />
      </svg>
    {/if}
    {#if playedPct != null && playhead != null && inView(playhead)}
      <div class="cue-detail-playhead" style:left="{playedPct}%"></div>
    {/if}
    <span class="cue-detail-edge start">{formatClock(frame.from)}</span>
    <span class="cue-detail-edge end">{formatClock(frame.to)}</span>
  </div>
</div>

<style lang="css">
  .cue-detail {
    display: flex;
    flex-direction: column;
  }

  .cue-lane {
    position: relative;
  }

  .cue-flag {
    position: absolute;
    height: 13px;
    padding: 0 4px;
    border: none;
    border-radius: 0 3px 3px 0;
    font-size: 9px;
    font-weight: 600;
    line-height: 13px;
    color: #0b0e14;
    background: var(--mk);
    cursor: ew-resize;
    white-space: nowrap;
    touch-action: none;
    opacity: 0.85;
  }

  .cue-flag.flip {
    transform: translateX(-100%);
    border-radius: 3px 0 0 3px;
  }

  .cue-flag.selected {
    opacity: 1;
    outline: 2px solid var(--on-surface);
    outline-offset: 0;
  }

  .cue-detail-curve {
    position: relative;
    height: 140px;
    background: var(--surface-container-lowest);
    border: 1px solid
      color-mix(in srgb, var(--outline-variant) 20%, transparent);
    border-radius: var(--r-lg, 4px);
    overflow: hidden;
  }

  .cue-detail-curve.seekable {
    cursor: pointer;
  }

  .cue-detail-bars,
  .cue-detail-envelope {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    pointer-events: none;
  }

  .cue-detail-bars .base rect {
    fill: var(--on-surface);
    opacity: 0.25;
  }

  .cue-detail-bars .played rect {
    fill: var(--secondary);
    opacity: 0.8;
  }

  .cue-detail-envelope polyline {
    fill: none;
    stroke: var(--on-surface);
    stroke-width: 1.5;
    opacity: 0.7;
  }

  .cue-detail-dim {
    position: absolute;
    top: 0;
    bottom: 0;
    background: color-mix(
      in srgb,
      var(--surface-container-lowest) 65%,
      transparent
    );
    pointer-events: none;
  }

  .cue-detail-line {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 2px;
    margin-left: -1px;
    background: var(--mk);
    pointer-events: none;
  }

  .cue-detail-line.selected {
    width: 3px;
    box-shadow: 0 0 6px var(--mk);
  }

  .cue-detail-playhead {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 1.5px;
    background: var(--error);
    box-shadow: 0 0 4px var(--error);
    pointer-events: none;
  }

  .cue-detail-edge {
    position: absolute;
    bottom: 2px;
    font-family: var(--font-mono);
    font-size: 10px;
    color: var(--on-surface-variant);
    pointer-events: none;
  }

  .cue-detail-edge.start {
    left: 4px;
  }

  .cue-detail-edge.end {
    right: 4px;
  }

  .cue-in {
    --mk: var(--cue-in-color);
  }
  .fade-in {
    --mk: var(--fade-in-color);
  }
  .fade-out {
    --mk: var(--fade-out-color);
  }
  .cue-out {
    --mk: var(--cue-out-color);
  }
  .next-start {
    --mk: var(--next-start-color);
  }
</style>
