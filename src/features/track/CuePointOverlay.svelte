<script lang="ts">
  /**
   * Cue point editor (#279). Opened from a library row's context menu, it is
   * the precise surface: the waveform full-width with draggable handles, plus a
   * millisecond field per marker for values a drag cannot hit.
   *
   * Auditioning happens **in here**. The dialog borrows the cue deck — loading
   * the draft with autoplay, driving its transport, drawing its playhead over
   * the curve — and hands it back on every exit. Nothing an operator hears
   * requires closing the dialog, and no draft is left armed behind a closed
   * one.
   *
   * Saving is a radio edit — it applies from the track's *next* airing, never
   * to the deck currently playing it, so on-air audio cannot re-decode under
   * the operator mid-broadcast. Clamping is the backend's: `saveCuePoints`
   * adopts whatever comes back rather than second-guessing it here.
   */
  import { untrack } from "svelte";
  import { app, formatTime, type CueSnapshot } from "../../shared/state.svelte";
  import { api } from "../../shared/api";
  import {
    CUE_MARKERS,
    NO_CUE_POINTS,
    cueMarkerPositions,
    cuePointsEqual,
    resolveCuePoints,
    type CueMarker,
  } from "../../shared/cuePoints";
  import type { CuePoints } from "../../shared/types";
  import Waveform from "../deck/Waveform.svelte";

  let overlay: HTMLDivElement | undefined = $state();
  let draft = $state<CuePoints>({ ...NO_CUE_POINTS });
  let peaks = $state<number[] | null>(null);
  let saving = $state(false);
  let error = $state<string | null>(null);
  /** What the cue deck held when the dialog opened, to give back on exit. */
  let deckBefore = $state<CueSnapshot | null>(null);
  let confirmingDiscard = $state(false);

  const track = $derived(app.editingCuePoints);
  const fileDuration = $derived(track?.duration ?? 0);
  const resolved = $derived(resolveCuePoints(draft, fileDuration));
  const airSeconds = $derived(Math.max(0, resolved.cueOut - resolved.cueIn));
  const dirty = $derived(
    track ? !cuePointsEqual(draft, track.cue_points) : false,
  );

  const markers = $derived(cueMarkerPositions(draft, fileDuration));

  /**
   * True when the cue deck is playing *this* draft. Editing a marker makes it
   * false, so the button returns to _Audition_ rather than resuming audio that
   * no longer matches what the dialog shows — reloading on every keystroke in a
   * millisecond field would be worse.
   */
  const auditioning = $derived(
    !!track &&
      app.cueTrack?.id === track.id &&
      cuePointsEqual(app.cueAppliedPoints, draft),
  );

  // The editor draws the whole file while the deck reports air time, so the
  // playhead has to be put back where cue-in moved it from.
  const auditionPct = $derived(
    auditioning && fileDuration > 0
      ? ((resolved.cueIn + app.cueCurrentTime) / fileDuration) * 100
      : 0,
  );

  $effect(() => {
    const t = app.editingCuePoints;
    if (!t) {
      draft = { ...NO_CUE_POINTS };
      peaks = null;
      error = null;
      confirmingDiscard = false;
      deckBefore = null;
      return;
    }
    draft = { ...NO_CUE_POINTS, ...(t.cue_points ?? {}) };
    error = null;
    confirmingDiscard = false;
    // Untracked: reading the deck's state reactively would re-run this effect
    // the moment an audition changes it, wiping the draft mid-edit.
    deckBefore = untrack(() => app.cueSnapshot());
    loadPeaks(t.id);
  });

  function loadPeaks(id: number): void {
    peaks = null;
    void api
      .getWaveform(id)
      .then((curve) => {
        if (app.editingCuePoints?.id === id) peaks = curve;
      })
      .catch(() => {
        // A missing curve is not an error here: the millisecond fields are the
        // authoritative control and work without one.
        peaks = null;
      });
  }

  /** A drag reports a fraction of the file; store it as whole milliseconds. */
  function onMarkerMove(id: string, at: number): void {
    if (!fileDuration) return;
    draft = { ...draft, [id]: Math.round(at * fileDuration * 1000) };
  }

  function onFieldInput(key: CueMarker, raw: string): void {
    const text = raw.trim();
    if (text === "") {
      draft = { ...draft, [key]: null };
      return;
    }
    const n = Number(text);
    if (!Number.isFinite(n)) return;
    draft = { ...draft, [key]: Math.max(0, Math.round(n)) };
  }

  /** Give an unset marker a position, so it gains a handle to drag. */
  function place(key: CueMarker): void {
    const at: Record<CueMarker, number> = {
      cue_in_ms: resolved.cueIn,
      fade_in_ms: resolved.fadeIn,
      fade_out_ms: resolved.fadeOut,
      cue_out_ms: resolved.cueOut,
      next_start_ms: resolved.nextStart,
    };
    draft = { ...draft, [key]: Math.round(at[key] * 1000) };
  }

  function clear(key: CueMarker): void {
    draft = { ...draft, [key]: null };
  }

  function clearAll(): void {
    draft = { ...NO_CUE_POINTS };
  }

  /**
   * Play the draft on the cue deck. Every press reloads it: markers are applied
   * at load time, so there is no way to change them on a running source.
   */
  function audition(): void {
    if (track) app.cueLoad(track, draft, true);
  }

  function toggleAudition(): void {
    if (!auditioning) {
      audition();
      return;
    }
    app.cueTogglePlay();
  }

  /** Park the audition at its start — not `cueStop`, which empties the deck. */
  function stopAudition(): void {
    if (!auditioning) return;
    if (app.cueIsPlaying) app.cueTogglePlay();
    app.cueSeekToPct(0);
  }

  /** A click on the curve seeks there, converting file time to air time. */
  function seekTo(at: number): void {
    if (!auditioning || !app.cueDuration) return;
    const airSeconds = at * fileDuration - resolved.cueIn;
    app.cueSeekToPct(airSeconds / app.cueDuration);
  }

  async function save(): Promise<void> {
    if (!track) return;
    saving = true;
    error = null;
    try {
      await app.saveCuePoints(track.id, draft);
      leave();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      saving = false;
    }
  }

  /**
   * Queue this draft as a single airing and leave the track untouched — the
   * other way out of the dialog, and the one that produces an item override.
   */
  function useOnce(): void {
    if (!track) return;
    app.queueCueDraft(track, draft);
    leave();
  }

  /** Hand the cue deck back and close. Every exit goes through here. */
  function leave(): void {
    if (deckBefore) app.cueRestore(deckBefore);
    app.editingCuePoints = null;
  }

  /**
   * Escape, the ×, the backdrop and _Cancel_ all land here. With changes
   * pending it asks first — the one thing an operator could not tell before was
   * whether closing kept them.
   */
  function requestClose(): void {
    if (dirty && !confirmingDiscard) {
      confirmingDiscard = true;
      return;
    }
    leave();
  }

  function onKeyDown(e: KeyboardEvent): void {
    if (e.key !== "Escape") return;
    e.preventDefault();
    requestClose();
  }

  $effect(() => {
    if (!app.editingCuePoints) return;
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  });

  /** Milliseconds as `m:ss.mmm` — the readout beside each field. */
  function stamp(ms: number | null): string {
    if (ms == null) return "—";
    const millis = Math.round(ms % 1000)
      .toString()
      .padStart(3, "0");
    return `${formatTime(ms / 1000)}.${millis}`;
  }
</script>

{#if track}
  <div
    class="cue-overlay"
    role="presentation"
    bind:this={overlay}
    onmousedown={(e) => {
      if (!overlay || e.target !== e.currentTarget) return;
      requestClose();
    }}
  >
    <div
      id="cue-point-dialog"
      class="cue-dialog"
      role="dialog"
      aria-modal="true"
      aria-label="Edit cue points"
      tabindex="-1"
    >
      <div class="cue-dialog-header">
        <div class="cue-dialog-heading">
          <h2>Cue Points</h2>
          <span class="cue-dialog-track">{track.title} — {track.artist}</span>
        </div>
        <button
          id="btn-cue-points-close"
          class="btn-close"
          onclick={requestClose}
          title="Close (Escape)"
          aria-label="Close"
        >
          <span class="material-symbols-outlined">close</span>
        </button>
      </div>

      <div class="cue-dialog-body">
        <div class="cue-canvas">
          <Waveform
            {peaks}
            progressPct={auditionPct}
            {markers}
            onmarkermove={onMarkerMove}
            onseek={app.cueDevice !== null ? seekTo : null}
            id="cue-editor"
          />
        </div>
        <p class="cue-summary">
          Airs <strong>{formatTime(airSeconds)}</strong> of
          {formatTime(fileDuration)}.
          {#if dirty}
            <span class="cue-dirty"
              >Unsaved. These changes are discarded unless you save them to the
              track or use them for one airing.</span
            >
          {:else}
            Saving applies these to every airing, from this track's next one.
          {/if}
        </p>

        <div class="cue-fields">
          {#each CUE_MARKERS as { key, kind, label, hint } (key)}
            <div class="cue-field">
              <span class="cue-swatch {kind}" aria-hidden="true"></span>
              <label class="cue-field-label" for="cue-field-{key}">
                {label}
                <span class="cue-field-hint">{hint}</span>
              </label>
              <input
                id="cue-field-{key}"
                type="number"
                min="0"
                step="1"
                inputmode="numeric"
                placeholder="—"
                value={draft[key] ?? ""}
                oninput={(e) => onFieldInput(key, e.currentTarget.value)}
              />
              <span class="cue-stamp">{stamp(draft[key])}</span>
              {#if draft[key] == null}
                <button class="btn-mini" onclick={() => place(key)}>Set</button>
              {:else}
                <button class="btn-mini" onclick={() => clear(key)}
                  >Clear</button
                >
              {/if}
            </div>
          {/each}
        </div>

        {#if error}
          <div id="cue-points-error" class="cue-error" role="alert">
            {error}
          </div>
        {/if}
      </div>

      <div class="cue-dialog-footer">
        <button id="btn-cue-points-clear" class="btn" onclick={clearAll}
          >Clear all</button
        >
        {#if app.cueDevice !== null}
          <div class="cue-audition" role="group" aria-label="Audition">
            <button
              id="btn-cue-points-audition"
              class="btn btn-icon"
              onclick={toggleAudition}
              title="Hear this edit on the cue deck"
            >
              <span class="material-symbols-outlined" aria-hidden="true"
                >{auditioning && app.cueIsPlaying
                  ? "pause"
                  : "play_arrow"}</span
              >
              {auditioning ? (app.cueIsPlaying ? "Pause" : "Play") : "Audition"}
            </button>
            <button
              id="btn-cue-points-stop"
              class="btn btn-icon"
              onclick={stopAudition}
              disabled={!auditioning}
              title="Back to the start of the audition"
              aria-label="Stop the audition"
            >
              <span class="material-symbols-outlined" aria-hidden="true"
                >stop</span
              >
            </button>
            <span class="cue-audition-clock"
              >{auditioning ? formatTime(app.cueCurrentTime) : "—:—"}</span
            >
          </div>
        {/if}
        <span class="cue-footer-gap"></span>
        {#if confirmingDiscard}
          <span class="cue-confirm">Discard these changes?</span>
          <button id="btn-cue-points-discard" class="btn" onclick={leave}
            >Discard</button
          >
          <button class="btn" onclick={() => (confirmingDiscard = false)}
            >Keep editing</button
          >
        {:else}
          <button
            id="btn-cue-points-use-once"
            class="btn"
            onclick={useOnce}
            disabled={saving}
            title="Queue this track next-up with these cue points, leaving the track itself untouched"
            >Use once</button
          >
          <button
            id="btn-cue-points-cancel"
            class="btn"
            onclick={requestClose}
            disabled={saving}>Cancel</button
          >
          <button
            id="btn-cue-points-save"
            class="btn btn-primary"
            onclick={save}
            disabled={saving || !dirty || !app.isAdmin}
            title={app.isAdmin
              ? "Every airing of this track uses these cue points"
              : "Unlock admin mode to save cue points to the track"}
          >
            {saving ? "Saving…" : "Save to track"}
          </button>
        {/if}
      </div>
    </div>
  </div>
{/if}

<style lang="css">
  .cue-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .cue-dialog {
    background: var(--surface-container, #1e2430);
    border-radius: var(--r-lg, 8px);
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.45);
    width: min(860px, calc(100% - 32px));
    max-height: 90vh;
    overflow-y: auto;
  }

  .cue-dialog-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
    padding: 16px 20px;
    border-bottom: 1px solid
      color-mix(in srgb, var(--outline-variant) 30%, transparent);
  }

  .cue-dialog-heading h2 {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
    color: var(--on-surface);
  }

  .cue-dialog-track {
    font-size: 12px;
    color: var(--on-surface-variant);
  }

  .btn-close {
    background: none;
    border: none;
    color: var(--on-surface-variant);
    cursor: pointer;
    padding: 4px;
    border-radius: 4px;
    display: flex;
  }

  .btn-close:hover {
    color: var(--on-surface);
    background: color-mix(in srgb, var(--on-surface) 10%, transparent);
  }

  .cue-dialog-body {
    padding: 20px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  /* The waveform is absolutely positioned inside its bar, so the canvas needs
     its own height — the decks get theirs from the deck body's row. */
  .cue-canvas {
    position: relative;
    height: 160px;
    background: var(--surface-container-lowest);
    border: 1px solid
      color-mix(in srgb, var(--outline-variant) 20%, transparent);
    border-radius: var(--r-lg, 4px);
    overflow: hidden;
  }

  .cue-summary {
    margin: 0;
    font-size: 12px;
    color: var(--on-surface-variant);
  }

  .cue-summary strong {
    color: var(--on-surface);
    font-variant-numeric: tabular-nums;
  }

  .cue-fields {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .cue-field {
    display: grid;
    grid-template-columns: 10px 1fr 120px 88px 64px;
    align-items: center;
    gap: 10px;
  }

  .cue-swatch {
    width: 10px;
    height: 10px;
    border-radius: 2px;
    background: var(--on-surface);
  }

  .cue-field-label {
    display: flex;
    flex-direction: column;
    font-size: 13px;
    color: var(--on-surface);
  }

  .cue-field-hint {
    font-size: 11px;
    color: var(--on-surface-variant);
  }

  .cue-field input {
    background: transparent;
    border: 1px solid
      color-mix(in srgb, var(--outline-variant) 40%, transparent);
    border-radius: 4px;
    padding: 6px 8px;
    color: var(--on-surface);
    font-family: var(--font-mono);
    font-size: 13px;
    outline: none;
    /* Keep the number field's spin buttons light-on-dark. */
    color-scheme: dark;
  }

  .cue-field input:focus {
    border-color: var(--primary);
  }

  .cue-stamp {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--on-surface-variant);
    font-variant-numeric: tabular-nums;
  }

  .cue-error {
    color: var(--error);
    font-size: 13px;
    padding: 8px 10px;
    background: color-mix(in srgb, var(--error) 12%, transparent);
    border-radius: 4px;
  }

  .cue-dialog-footer {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 16px 20px;
    border-top: 1px solid
      color-mix(in srgb, var(--outline-variant) 30%, transparent);
  }

  .cue-footer-gap {
    flex: 1;
  }

  .cue-audition {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .cue-audition-clock {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--on-surface-variant);
    font-variant-numeric: tabular-nums;
  }

  .cue-confirm {
    font-size: 13px;
    color: var(--on-surface);
  }

  .cue-dirty {
    color: var(--cue-out-color);
  }

  .btn,
  .btn-mini {
    background: transparent;
    border: 1px solid
      color-mix(in srgb, var(--outline-variant) 40%, transparent);
    color: var(--on-surface-variant);
    border-radius: 4px;
    cursor: pointer;
    font-size: 13px;
    padding: 8px 16px;
  }

  .btn-mini {
    padding: 4px 8px;
    font-size: 11px;
  }

  .btn-icon {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
  }

  .btn-icon .material-symbols-outlined {
    font-size: 18px;
  }

  .btn:hover:not(:disabled),
  .btn-mini:hover {
    background: color-mix(in srgb, var(--on-surface) 8%, transparent);
    color: var(--on-surface);
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-primary {
    background: var(--primary);
    border-color: var(--primary);
    color: var(--on-primary);
  }

  .cue-swatch.cue-in {
    background: var(--cue-in-color);
  }
  .cue-swatch.fade-in {
    background: var(--fade-in-color);
  }
  .cue-swatch.fade-out {
    background: var(--fade-out-color);
  }
  .cue-swatch.cue-out {
    background: var(--cue-out-color);
  }
  .cue-swatch.next-start {
    background: var(--next-start-color);
  }
</style>
