<script lang="ts">
  import { app, formatTime } from "../../shared/state.svelte";
  import { airDuration, isTrimmed } from "../../shared/cuePoints";
  import { formatKey } from "../../shared/camelot";
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

  /**
   * Below this, the measurement is shown as one the estimator was unsure of
   * rather than hidden: a weak reading on a spoken-word track is information,
   * and nothing in the app acts on the number.
   */
  const WEAK_CONFIDENCE = 0.3;

  function formatRate(hz: number | null | undefined): string {
    return hz ? `${(hz / 1000).toFixed(1)} kHz` : UNKNOWN;
  }

  function formatBitrate(bps: number | null | undefined): string {
    return bps ? `${Math.round(bps / 1000)} kbps` : UNKNOWN;
  }

  /** The measured tempo, noted as weak when little stood behind it. */
  function formatMeasured(t: Track): string | null {
    if (t.detected_bpm == null) return null;
    const bpm = t.detected_bpm.toFixed(1);
    return (t.bpm_confidence ?? 1) < WEAK_CONFIDENCE ? `${bpm} (weak)` : bpm;
  }

  /**
   * The measured key with its Camelot code, noted as weak on the same terms the
   * tempo is. The tag's own key keeps its row above: the two disagree often
   * enough to be worth seeing, and a tagger's value is not this app's to correct.
   */
  function formatMeasuredKey(t: Track): string | null {
    const key = formatKey(t.detected_key);
    if (!key) return null;
    return (t.key_confidence ?? 1) < WEAK_CONFIDENCE ? `${key} (weak)` : key;
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

  /** `3 of 12` when the tag carried a total, plain `3` when it did not. */
  function formatPosition(
    no: number | null | undefined,
    total: number | null | undefined,
  ): string | null {
    if (no == null) return null;
    return total == null ? String(no) : `${no} of ${total}`;
  }

  /**
   * A comment can be a paragraph. The tooltip positions itself from its own
   * height, so an unbounded one would push it off the bottom of the screen with
   * nothing to scroll — and the whole value is on every row a search returns.
   */
  function shorten(text: string | null | undefined): string | null {
    if (!text || !text.trim()) return null;
    const flat = text.trim().replace(/\s+/g, " ");
    return flat.length > COMMENT_MAX ? `${flat.slice(0, COMMENT_MAX)}…` : flat;
  }

  const COMMENT_MAX = 140;

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
  <!-- The always-present rows read Unknown when a file lacks them, which is
       information: those are the fields a track is expected to have. The newer
       ones are absent far more often than not, so they are dropped instead —
       nine Unknowns would bury the rows that matter. -->
  {@const fields = [
    { label: "Album", value: strOr(t.album) },
    { label: "Album artist", value: t.album_artist ?? null },
    { label: "Track", value: formatPosition(t.track_no, t.track_total) },
    { label: "Disc", value: formatPosition(t.disc_no, t.disc_total) },
    { label: "Genre", value: strOr(t.genre) },
    { label: "Year", value: numOr(t.year) },
    { label: "Duration", value: formatDuration(t) },
    { label: "BPM", value: numOr(t.bpm) },
    { label: "Measured BPM", value: formatMeasured(t) },
    { label: "Key", value: t.initial_key ?? null },
    { label: "Measured key", value: formatMeasuredKey(t) },
    { label: "Format", value: t.format ? t.format.toUpperCase() : UNKNOWN },
    { label: "Bitrate", value: formatBitrate(t.bitrate) },
    { label: "Sample rate", value: formatRate(t.sample_rate) },
    { label: "ISRC", value: t.isrc ?? null },
    { label: "Plays", value: String(t.play_count ?? 0) },
    { label: "Comment", value: shorten(t.comment) },
  ].filter((f): f is { label: string; value: string } => f.value !== null)}
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
