<script lang="ts" module>
  import type { CueMarkerKind } from "../../shared/cuePoints";

  /** A cue-point marker drawn over the curve, optionally draggable. */
  export interface WaveformMarker {
    /** Stable key; handed back on drag. */
    id: string;
    /** Position as a fraction (0..1) of the whole file. */
    at: number;
    /** Drives the marker colour; see `.wf-marker` in styles.css. */
    kind: CueMarkerKind;
    /** Native tooltip on the grab area. */
    label: string;
  }
</script>

<script lang="ts">
  /**
   * Amplitude-curve overlay for a deck seek bar. The stored peak curve is
   * symmetric, so only the top half is drawn: bars rise from the bar's baseline,
   * giving the DJ a visual map of song structure to seek against. A dim layer
   * covers the whole track; an accent layer clipped to the played portion tracks
   * the playhead.
   *
   * Bars are drawn once per track (they depend only on `peaks`); only the clip
   * width tracks `progressPct` as playback advances, so the 100 ms time tick
   * stays cheap. `height`/`width` are pinned to 100% so the SVG fills its bar
   * instead of falling back to the viewBox's intrinsic aspect ratio.
   *
   * Two cue-point features ride on the same geometry (#279). `crop` narrows the
   * viewBox to a sub-range of the file, which is how *Preview* mode shows only
   * what airs; `markers` draws the cue points, draggable when `onmarkermove` is
   * given. The SVG stays `aria-hidden`: dragging is a pointer convenience, and
   * the cue editor's millisecond fields are the accessible way to set a marker.
   */
  interface Props {
    peaks: number[] | null;
    /** Playback position as a percentage of the *displayed* span. */
    progressPct: number;
    /** Cursor position (0..100) while hovering the bar, or null when away. */
    hoverPct?: number | null;
    /** Unique per instance — clipPath ids must not collide across decks. */
    id: string;
    /** Fraction of the file to draw; the whole file when null. */
    crop?: { from: number; to: number } | null;
    /** Cue points to draw over the curve. */
    markers?: WaveformMarker[] | null;
    /** Makes the markers draggable. Reports a fraction of the whole file. */
    onmarkermove?: ((id: string, at: number) => void) | null;
  }

  const {
    peaks,
    progressPct,
    hoverPct = null,
    id,
    crop = null,
    markers = null,
    onmarkermove = null,
  }: Props = $props();

  // viewBox units: one x-unit per bucket, 0..100 vertical (bars grow up from the
  // baseline at y=100). preserveAspectRatio "none" stretches the grid to the
  // bar's real pixel size.
  const HEIGHT = 100;
  const MIN_BAR = 3; // keep silent buckets visible as a thin baseline
  /** Grid width used when there is no curve, so markers still have a scale. */
  const NO_PEAKS_UNITS = 100;

  const count = $derived(peaks?.length ?? 0);
  const units = $derived(count || NO_PEAKS_UNITS);
  const bars = $derived(
    (peaks ?? []).map((v, i) => {
      const h = Math.max(MIN_BAR, (v / 255) * HEIGHT);
      return { x: i, y: HEIGHT - h, h };
    }),
  );

  // The drawn window. A zero-width or inverted crop is ignored rather than
  // collapsing the viewBox, which would blank the deck on a degenerate region.
  const from = $derived(crop && crop.to > crop.from ? clamp01(crop.from) : 0);
  const to = $derived(crop && crop.to > crop.from ? clamp01(crop.to) : 1);
  const originX = $derived(from * units);
  const spanX = $derived(Math.max(1e-6, (to - from) * units));

  const playedX = $derived(originX + clamp01(progressPct / 100) * spanX);
  const hoverX = $derived(
    hoverPct == null ? null : originX + clamp01(hoverPct / 100) * spanX,
  );
  const clipId = $derived(`wf-clip-${id}`);

  function clamp01(v: number): number {
    return Math.min(1, Math.max(0, v));
  }

  // ----- Marker dragging -----

  let svg: SVGSVGElement | undefined = $state();
  let dragging: string | null = null;

  /** Map a viewport x to a fraction of the *file*, inside the drawn window. */
  function atFromClientX(clientX: number): number {
    if (!svg) return from;
    const rect = svg.getBoundingClientRect();
    const t = clamp01((clientX - rect.left) / rect.width);
    return from + t * (to - from);
  }

  // Every handler stops propagation: the seek bar underneath treats a
  // pointerdown as a scrub, and grabbing a handle must not also move the
  // playhead.
  function onHandleDown(e: PointerEvent, markerId: string): void {
    if (!onmarkermove) return;
    e.stopPropagation();
    e.preventDefault();
    dragging = markerId;
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
    onmarkermove(markerId, atFromClientX(e.clientX));
  }

  function onHandleMove(e: PointerEvent): void {
    if (!dragging) return;
    e.stopPropagation();
    onmarkermove?.(dragging, atFromClientX(e.clientX));
  }

  function onHandleUp(e: PointerEvent): void {
    if (!dragging) return;
    e.stopPropagation();
    (e.currentTarget as Element).releasePointerCapture(e.pointerId);
    dragging = null;
  }
</script>

<svg
  class="waveform"
  class:editable={onmarkermove != null}
  bind:this={svg}
  viewBox="{originX} 0 {spanX} {HEIGHT}"
  preserveAspectRatio="none"
  aria-hidden="true"
>
  {#if count > 0}
    <defs>
      <clipPath id={clipId}>
        <rect
          x={originX}
          y="0"
          width={Math.max(0, playedX - originX)}
          height={HEIGHT}
        />
      </clipPath>
    </defs>
    <g class="wf-base">
      {#each bars as b (b.x)}
        <rect x={b.x + 0.1} y={b.y} width="0.8" height={b.h} />
      {/each}
    </g>
    <g class="wf-played" clip-path="url(#{clipId})">
      {#each bars as b (b.x)}
        <rect x={b.x + 0.1} y={b.y} width="0.8" height={b.h} />
      {/each}
    </g>
  {:else}
    <!-- No waveform (e.g. jingles, or nothing loaded): a flat centered line,
         matching the mockup's zero-data deck. -->
    <line
      class="wf-flatline"
      x1={originX}
      y1={HEIGHT / 2}
      x2={originX + spanX}
      y2={HEIGHT / 2}
      vector-effect="non-scaling-stroke"
    />
  {/if}
  <!-- Glowing red playhead at the current play position. -->
  <line
    class="wf-playhead"
    x1={playedX}
    y1="0"
    x2={playedX}
    y2={HEIGHT}
    vector-effect="non-scaling-stroke"
  />
  {#if hoverX != null}
    <!-- Seek preview marker: a 1px line at the cursor (non-scaling so it stays
         crisp under the preserveAspectRatio="none" stretch). -->
    <line
      class="wf-cursor"
      x1={hoverX}
      y1="0"
      x2={hoverX}
      y2={HEIGHT}
      vector-effect="non-scaling-stroke"
    />
  {/if}
  {#if markers}
    <g class="wf-markers">
      {#each markers as m (m.id)}
        {@const x = clamp01(m.at) * units}
        <line
          class="wf-marker {m.kind}"
          x1={x}
          y1="0"
          x2={x}
          y2={HEIGHT}
          vector-effect="non-scaling-stroke"
        />
        {#if onmarkermove}
          <!-- Transparent fat line over the marker: a constant-width grab area
               that survives the non-uniform stretch, where a <rect> would not. -->
          <line
            class="wf-grab"
            x1={x}
            y1="0"
            x2={x}
            y2={HEIGHT}
            vector-effect="non-scaling-stroke"
            role="presentation"
            onpointerdown={(e) => onHandleDown(e, m.id)}
            onpointermove={onHandleMove}
            onpointerup={onHandleUp}
            onpointercancel={onHandleUp}
          >
            <title>{m.label}</title>
          </line>
        {/if}
      {/each}
    </g>
  {/if}
</svg>
