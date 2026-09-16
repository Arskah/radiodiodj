<script lang="ts">
  import { app, formatSpan, formatTime } from "../../shared/state.svelte";
  import Waveform from "./Waveform.svelte";
  import MissingBadge from "../track/MissingBadge.svelte";
  import defaultCover from "../../assets/radiodiodi_label.svg";

  let progressBar: HTMLDivElement;
  let hoverPct = $state<number | null>(null);

  function pctFromClientX(clientX: number): number {
    const rect = progressBar.getBoundingClientRect();
    return ((clientX - rect.left) / rect.width) * 100;
  }

  function onDoubleClick(e: MouseEvent): void {
    app.seekToPct(pctFromClientX(e.clientX) / 100);
  }

  function onPointerMove(e: PointerEvent): void {
    hoverPct = pctFromClientX(e.clientX);
  }

  function onPointerLeave(): void {
    hoverPct = null;
  }
</script>

<section id="now-playing" class="deck inner-shadow-recessed">
  <div id="player-info">
    <div id="track-info">
      <div class="deck-head">
        <span class="material-symbols-outlined" aria-hidden="true"
          >cell_tower</span
        >
        <span class="deck-label">Main Deck</span>
        {#if app.currentTrack}
          <MissingBadge trackId={app.currentTrack.id} />
        {/if}
        {#if app.airTimeRemaining !== null}
          <span
            class="deck-remaining"
            title={app.autoAdvance
              ? "On air until the queue runs out or reaches a stop marker"
              : "On air until this track ends — Manual does not advance"}
            >−{formatSpan(app.airTimeRemaining)}</span
          >
        {/if}
      </div>
      <div class="track-names">
        <span id="np-title"
          >{app.currentTrack ? app.currentTrack.title : "No Track Loaded"}</span
        >
        <span id="np-artist">{app.currentTrack?.artist ?? ""}</span>
        {#if app.isBuffering}
          <!-- Shimmer on the progress bar is the visual cue; keep a
               screen-reader-only announcement since CSS is invisible to AT. -->
          <span class="sr-only" aria-live="polite">Buffering…</span>
        {/if}
      </div>
    </div>
    <div class="deck-header-controls">
      <div id="player-controls">
        <button
          id="btn-prev"
          class="transport"
          title="Previous"
          aria-label="Previous track"
          onclick={() => app.prev()}
        >
          <span class="material-symbols-outlined">skip_previous</span>
        </button>
        <button
          id="btn-play"
          class="transport transport-primary"
          title={app.isPlaying ? "Pause" : "Play"}
          aria-label={app.isPlaying ? "Pause" : "Play"}
          aria-pressed={app.isPlaying}
          onclick={() => app.togglePlay()}
        >
          <span class="material-symbols-outlined"
            >{app.isPlaying ? "pause" : "play_arrow"}</span
          >
        </button>
        <button
          id="btn-stop"
          class="transport"
          title="Stop"
          aria-label="Stop"
          onclick={() => app.stop()}
        >
          <span class="material-symbols-outlined">stop</span>
        </button>
        <button
          id="btn-next"
          class="transport"
          title="Next"
          aria-label="Next track"
          onclick={() => app.next()}
        >
          <span class="material-symbols-outlined">skip_next</span>
        </button>
      </div>
      <div
        class="segmented"
        role="group"
        aria-label="Auto/Manual playback mode"
      >
        <button
          class:active={app.autoAdvance}
          aria-pressed={app.autoAdvance}
          onclick={() => {
            if (!app.autoAdvance) app.toggleMode();
          }}>Auto</button
        >
        <button
          class:active={!app.autoAdvance}
          aria-pressed={!app.autoAdvance}
          onclick={() => {
            if (app.autoAdvance) app.toggleMode();
          }}>Manual</button
        >
      </div>
    </div>
  </div>
  <div class="deck-body">
    <!-- Rotating vinyl disc: cover art in the center label, spinning while the
         deck plays (#271). Falls back to a cream-label RadiodioDJ mark (matching
         the app icon) when the track has no embedded artwork. -->
    <div class="vinyl-disc" class:spinning={app.isPlaying} aria-hidden="true">
      <img class="vinyl-art" src={app.coverArt ?? defaultCover} alt="" />
    </div>
    <div
      id="progress-bar"
      class:buffering={app.isBuffering}
      bind:this={progressBar}
      role="slider"
      tabindex="0"
      title="Double-click to seek"
      aria-label="Seek (double-click)"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(app.progressPct)}
      ondblclick={onDoubleClick}
      onpointermove={onPointerMove}
      onpointerleave={onPointerLeave}
    >
      <Waveform
        peaks={app.waveform}
        progressPct={app.progressPct}
        {hoverPct}
        id="main"
      />
      <div id="progress-fill" style:width="{app.progressPct}%"></div>
      <span class="time-pill"
        >{formatTime(app.currentTime)} / {formatTime(app.duration)}</span
      >
    </div>
  </div>
</section>
