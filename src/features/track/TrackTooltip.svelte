<script lang="ts">
  import { app, formatTime } from "../../shared/state.svelte";
  import { airDuration, isTrimmed } from "../../shared/cuePoints";
  import type { Track } from "../../shared/types";

  const TOOLTIP_WIDTH = 280;
  const GAP = 8;

  let tooltip: HTMLDivElement | undefined = $state();
  let height = $state(0);

  $effect(() => {
    if (app.hoveredTrack && tooltip) {
      height = tooltip.offsetHeight;
    }
  });

  let left = $derived.by(() => {
    const x = app.hoverX + GAP;
    const max = window.innerWidth - TOOLTIP_WIDTH - GAP;
    if (x > max) return Math.max(GAP, app.hoverX - TOOLTIP_WIDTH - GAP);
    return x;
  });

  let top = $derived.by(() => {
    const max = window.innerHeight - height - GAP;
    return Math.min(Math.max(GAP, app.hoverY), max);
  });

  const UNKNOWN = "Unknown";

  function formatRate(hz: number | null | undefined): string {
    return hz ? `${(hz / 1000).toFixed(1)} kHz` : UNKNOWN;
  }

  function formatBitrate(bps: number | null | undefined): string {
    return bps ? `${Math.round(bps / 1000)} kbps` : UNKNOWN;
  }

  function strOr(v: string | null | undefined): string {
    return v && v.trim() ? v : UNKNOWN;
  }

  function numOr(v: number | null | undefined): string {
    return v != null ? String(v) : UNKNOWN;
  }

  function isUnknown(v: string): boolean {
    return v === UNKNOWN;
  }

  /**
   * What the track airs, and — when cue points trim it — the file length too,
   * so the shorter number in the library column is explained rather than
   * mysterious.
   */
  function formatDuration(t: Track): string {
    if (!t.duration) return UNKNOWN;
    const air = formatTime(airDuration(t));
    return isTrimmed(t) ? `Airs ${air} · File ${formatTime(t.duration)}` : air;
  }
</script>

{#if app.hoveredTrack}
  {@const t = app.hoveredTrack}
  {@const fields = [
    { label: "Album", value: strOr(t.album) },
    { label: "Genre", value: strOr(t.genre) },
    { label: "Year", value: numOr(t.year) },
    { label: "Duration", value: formatDuration(t) },
    { label: "BPM", value: numOr(t.bpm) },
    { label: "Format", value: t.format ? t.format.toUpperCase() : UNKNOWN },
    { label: "Bitrate", value: formatBitrate(t.bitrate) },
    { label: "Sample rate", value: formatRate(t.sample_rate) },
    { label: "Plays", value: String(t.play_count ?? 0) },
  ]}
  <div
    class="track-tooltip"
    bind:this={tooltip}
    style:left="{left}px"
    style:top="{top}px"
    style:width="{TOOLTIP_WIDTH}px"
    role="tooltip"
  >
    <div class="tt-title">{strOr(t.title)}</div>
    <div class="tt-artist" class:unknown={isUnknown(strOr(t.artist))}>
      {strOr(t.artist)}
    </div>
    <dl class="tt-meta">
      {#each fields as { label, value } (label)}
        <dt>{label}</dt>
        <dd class:unknown={isUnknown(value)}>{value}</dd>
      {/each}
    </dl>
  </div>
{/if}
