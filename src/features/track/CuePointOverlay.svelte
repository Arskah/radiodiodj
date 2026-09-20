<script lang="ts">
  /**
   * Cue point editor (#279). Markers are placed roughly by eye, then tuned by
   * ear:
   *
   * - The **overview** strip shows the whole file. A click seeks; Cue In and
   *   Cue Out drag by the tabs on the region's edges.
   * - The **detail** strip zooms onto the region plus a margin, or follows the
   *   playhead until there is one. Markers drag by their flags in the lane
   *   above it, never by the curve, so a seek click cannot move a marker.
   *   Sizing the overview's zoom box by hand takes framing over until _Fit_.
   * - **Play** plays the raw file and ignores edits; **Audition** plays the
   *   draft, and an edit mid-audition reloads it where it was.
   * - Each row, and the keyboard, marks at the playhead and nudges.
   *
   * The dialog borrows the cue deck and hands it back on every exit, so no
   * draft is left armed behind a closed one. Saving is a radio edit: it applies
   * from the track's *next* airing, never to a deck already playing it.
   */
  import { untrack } from "svelte";
  import { app, formatTime, type CueSnapshot } from "../../shared/state.svelte";
  import { api } from "../../shared/api";
  import {
    CUE_MARKERS,
    NO_CUE_POINTS,
    cuePointsEqual,
    hasCuePoints,
    resolveCuePoints,
    type CueMarker,
  } from "../../shared/cuePoints";
  import {
    MARK_KEYS,
    clearMarker,
    editorFrame,
    envelopePoints,
    followFrame,
    followNeedsPage,
    formatClock,
    hasRegion,
    needsReframe,
    nudge,
    panFrame,
    nudgeStep,
    playheadFile,
    preRollTarget,
    reloadStartAt,
    resizeFrame,
    resolvedMs,
    setMarker,
    type Frame,
  } from "../../shared/cueEditor";
  import type { CuePoints } from "../../shared/types";
  import type { WaveformMarker } from "../deck/Waveform.svelte";
  import CueOverview from "./CueOverview.svelte";
  import CueDetail, { type DetailMarker } from "./CueDetail.svelte";
  import CueMarkerRow from "./CueMarkerRow.svelte";

  /** Seconds per bucket of `api.getWaveformDetail`. */
  const DETAIL_STEP = 0.01;
  /** How long an audition waits after the last edit before reloading. */
  const RELOAD_DEBOUNCE_MS = 250;

  const SHORT: Record<CueMarker, string> = {
    cue_in_ms: "In",
    fade_in_ms: "FI",
    fade_out_ms: "FO",
    cue_out_ms: "Out",
    next_start_ms: "NS",
  };

  const FALLBACK: Record<CueMarker, string> = {
    cue_in_ms: "= start",
    fade_in_ms: "= Cue In",
    fade_out_ms: "= Cue Out",
    cue_out_ms: "= end",
    next_start_ms: "= Cue Out",
  };

  let overlay: HTMLDivElement | undefined = $state();
  let draft = $state<CuePoints>({ ...NO_CUE_POINTS });
  let peaks = $state<number[] | null>(null);
  let detail = $state<Uint8Array | null>(null);
  let saving = $state(false);
  let error = $state<string | null>(null);
  /** What the cue deck held when the dialog opened, to give back on exit. */
  let deckBefore = $state<CueSnapshot | null>(null);
  let confirmingDiscard = $state(false);
  let selected = $state<CueMarker | null>(null);
  let frame = $state<Frame>({ from: 0, to: 0 });
  /** Whether `frame` is framing a region (vs. following the playhead). */
  let framingRegion = $state(false);
  /** A handle is being dragged: the frame holds and auditions wait. */
  let dragging = $state(false);
  /** The zoom was sized by hand, so it no longer follows the region. */
  let manualFrame = $state(false);

  const track = $derived(app.editingCuePoints);
  const fileDuration = $derived(track?.duration ?? 0);
  const resolved = $derived(resolveCuePoints(draft, fileDuration));
  const airSeconds = $derived(Math.max(0, resolved.cueOut - resolved.cueIn));
  const dirty = $derived(
    track ? !cuePointsEqual(draft, track.cue_points) : false,
  );
  const canPlay = $derived(app.cueDevice !== null);

  /** The deck is playing this track: raw (whole file) or an audition. */
  const onDeck = $derived(!!track && app.cueTrack?.id === track.id);
  const mode = $derived<"raw" | "audition" | null>(
    !onDeck ? null : app.cueAppliedPoints === null ? "raw" : "audition",
  );
  /** Where the deck is, in file seconds; null when it isn't on this track. */
  const playhead = $derived(
    onDeck
      ? playheadFile(app.cueAppliedPoints, fileDuration, app.cueCurrentTime)
      : null,
  );
  const auditionCurrent = $derived(
    mode === "audition" && cuePointsEqual(app.cueAppliedPoints, draft),
  );

  const region = $derived<Frame | null>(
    hasRegion(draft) ? { from: resolved.cueIn, to: resolved.cueOut } : null,
  );

  const setMarkers = $derived(
    CUE_MARKERS.filter(({ key }) => draft[key] != null),
  );

  const overviewMarkers = $derived<WaveformMarker[]>(
    fileDuration > 0
      ? setMarkers
          .filter(({ key }) => key !== "cue_in_ms" && key !== "cue_out_ms")
          .map(({ key, kind }) => ({
            id: key,
            kind,
            at: (draft[key] as number) / 1000 / fileDuration,
          }))
      : [],
  );

  const detailMarkers = $derived<DetailMarker[]>(
    setMarkers.map(({ key, kind, label }) => ({
      id: key,
      kind,
      label,
      short: SHORT[key],
      t: (draft[key] as number) / 1000,
    })),
  );

  const envelope = $derived(
    hasCuePoints(draft) && fileDuration > 0
      ? envelopePoints(draft, fileDuration, frame)
      : null,
  );

  // The editor owns the draft; the store owns what lands from the analysis
  // pass, and needs to know whether refreshing the markers would take work
  // away from the operator.
  $effect(() => {
    app.cueEditorDirty = dirty;
  });

  // ----- Opening and closing -----

  $effect(() => {
    const t = app.editingCuePoints;
    if (!t) {
      draft = { ...NO_CUE_POINTS };
      peaks = null;
      detail = null;
      error = null;
      confirmingDiscard = false;
      deckBefore = null;
      selected = null;
      dragging = false;
      return;
    }
    draft = { ...NO_CUE_POINTS, ...(t.cue_points ?? {}) };
    error = null;
    confirmingDiscard = false;
    selected = null;
    // Untracked: reading the deck reactively would re-run this effect the
    // moment playback changes it, wiping the draft mid-edit.
    untrack(() => {
      deckBefore = app.cueSnapshot();
      manualFrame = false;
      reframe(true);
    });
    loadCurves(t.id);
  });

  function loadCurves(id: number): void {
    peaks = null;
    detail = null;
    // A missing curve is not an error here: the time fields work without one.
    void api
      .getWaveform(id)
      .then((curve) => {
        if (app.editingCuePoints?.id === id) peaks = curve;
      })
      .catch(() => {});
    void api
      .getWaveformDetail(id)
      .then((curve) => {
        if (app.editingCuePoints?.id === id) detail = curve;
      })
      .catch(() => {});
  }

  // ----- Framing -----

  /**
   * Point the detail strip at the region, or at the playhead when there is
   * none. Unforced, it only moves when the region has come or gone, or no
   * longer fits the frame.
   */
  function reframe(force = false): void {
    if (!track || fileDuration <= 0) return;
    // A hand-sized zoom outranks every automatic refit; only Fit (and
    // reopening the dialog) gives framing back.
    if (manualFrame) return;
    const nowRegion = hasRegion(draft);
    if (
      force ||
      nowRegion !== framingRegion ||
      (nowRegion && needsReframe(frame, draft, fileDuration))
    ) {
      frame = editorFrame(draft, fileDuration, playhead);
      framingRegion = nowRegion;
    }
  }

  /** Hand framing back to the region — the _Fit_ button and a box double-click. */
  function fitFrame(): void {
    manualFrame = false;
    reframe(true);
  }

  function resizeZoom(edge: "from" | "to", t: number): void {
    manualFrame = true;
    frame = resizeFrame(frame, edge, t, fileDuration);
  }

  function panZoom(t: number): void {
    manualFrame = true;
    frame = panFrame(frame, t, fileDuration);
  }

  // A following frame turns the page as the playhead reaches its edge.
  $effect(() => {
    if (manualFrame || framingRegion || playhead == null || fileDuration <= 0) {
      return;
    }
    if (untrack(() => followNeedsPage(frame, playhead, fileDuration))) {
      frame = followFrame(playhead, fileDuration);
    }
  });

  // ----- Editing -----

  function edit(next: CuePoints, force = false): void {
    draft = next;
    if (!dragging) reframe(force);
  }

  function select(key: CueMarker): void {
    selected = key;
  }

  function moveTo(key: CueMarker, t: number): void {
    dragging = true;
    selected = key;
    draft = setMarker(draft, key, t * 1000, fileDuration);
  }

  function endDrag(): void {
    dragging = false;
    reframe(true);
  }

  /** Put `key` at the playhead — or, with nothing playing, where it resolves. */
  function mark(key: CueMarker): void {
    const ms =
      playhead != null ? playhead * 1000 : resolvedMs(draft, key, fileDuration);
    selected = key;
    edit(setMarker(draft, key, ms, fileDuration));
  }

  function nudgeMarker(key: CueMarker, deltaMs: number): void {
    selected = key;
    edit(nudge(draft, key, deltaMs, fileDuration));
  }

  function commitField(key: CueMarker, ms: number | null): void {
    edit(
      ms == null
        ? clearMarker(draft, key)
        : setMarker(draft, key, ms, fileDuration),
      true,
    );
  }

  function clearAll(): void {
    edit({ ...NO_CUE_POINTS }, true);
  }

  // ----- Playback -----

  /** Play or pause the raw file, starting where the playhead is. */
  function playRaw(): void {
    if (!track || !canPlay) return;
    if (mode === "raw") {
      app.cueTogglePlay();
      return;
    }
    app.cueLoad(track, null, true, playhead ?? 0);
  }

  /** Play or pause the draft, loading it first if the deck has anything else. */
  function audition(): void {
    if (!track || !canPlay) return;
    if (auditionCurrent) {
      app.cueTogglePlay();
      return;
    }
    const at =
      playhead != null ? reloadStartAt(playhead, draft, fileDuration) : 0;
    app.cueLoad(track, { ...draft }, true, at);
  }

  function togglePlay(): void {
    if (mode === "audition") audition();
    else playRaw();
  }

  /** Park at the start of whatever is loaded — not `cueStop`, which empties it. */
  function stop(): void {
    if (!mode) return;
    if (app.cueIsPlaying) app.cueTogglePlay();
    app.cueSeekToPct(0);
  }

  /**
   * Seek to file time `t`. With nothing loaded this parks the raw file there,
   * so there is a playhead to mark at before anything has played.
   */
  function seek(t: number): void {
    if (!track || !canPlay) return;
    if (mode === null) {
      app.cueLoad(track, null, false, t);
      return;
    }
    if (!app.cueDuration) return;
    const offset =
      mode === "raw"
        ? 0
        : resolveCuePoints(app.cueAppliedPoints, fileDuration).cueIn;
    app.cueSeekToPct((t - offset) / app.cueDuration);
  }

  function preRoll(key: CueMarker): void {
    if (!track || !canPlay) return;
    const target = preRollTarget(key, draft, fileDuration);
    app.cueLoad(
      track,
      target.mode === "raw" ? null : { ...draft },
      true,
      target.startAt,
    );
  }

  // An edit during an audition reloads it where it was, once the drag has
  // dropped and typing or nudging has paused.
  $effect(() => {
    const d = draft;
    if (dragging) return;
    const stale = untrack(
      () => mode === "audition" && !cuePointsEqual(app.cueAppliedPoints, d),
    );
    if (!stale) return;
    const timer = setTimeout(() => {
      untrack(() => {
        if (!track || mode !== "audition") return;
        const at =
          playhead != null ? reloadStartAt(playhead, draft, fileDuration) : 0;
        app.cueLoad(track, { ...draft }, app.cueIsPlaying, at);
      });
    }, RELOAD_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  });

  // ----- Exits -----

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
   * pending it asks first.
   */
  function requestClose(): void {
    if (dirty && !confirmingDiscard) {
      confirmingDiscard = true;
      return;
    }
    leave();
  }

  // ----- Keyboard -----

  function typing(target: EventTarget | null): boolean {
    return (
      target instanceof HTMLInputElement ||
      target instanceof HTMLTextAreaElement ||
      target instanceof HTMLSelectElement
    );
  }

  function onKeyDown(e: KeyboardEvent): void {
    if (e.key === "Escape") {
      e.preventDefault();
      requestClose();
      return;
    }
    if (
      typing(e.target) ||
      e.metaKey ||
      e.ctrlKey ||
      (e.repeat && e.key === " ")
    ) {
      return;
    }
    const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
    // `e.code` for letters: Alt changes `e.key` on macOS.
    const letter =
      /^Key([A-Z])$/.exec(e.code)?.[1]?.toLowerCase() ??
      (/^[a-z]$/.test(key) ? key : undefined);
    if (key === " ") {
      togglePlay();
    } else if (letter === "a") {
      audition();
    } else if (letter === "p" && selected) {
      preRoll(selected);
    } else if (letter && MARK_KEYS[letter]) {
      mark(MARK_KEYS[letter]);
    } else if ((key === "ArrowLeft" || key === "ArrowRight") && selected) {
      const step = nudgeStep(e);
      nudgeMarker(selected, key === "ArrowLeft" ? -step : step);
    } else {
      return;
    }
    e.preventDefault();
  }

  /** Space would otherwise also click whichever button has focus. */
  function onKeyUp(e: KeyboardEvent): void {
    if (e.key === " " && !typing(e.target)) e.preventDefault();
  }

  $effect(() => {
    if (!app.editingCuePoints) return;
    document.addEventListener("keydown", onKeyDown);
    document.addEventListener("keyup", onKeyUp);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      document.removeEventListener("keyup", onKeyUp);
    };
  });
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
        <CueOverview
          {peaks}
          {fileDuration}
          {region}
          markers={overviewMarkers}
          {frame}
          {playhead}
          manual={manualFrame}
          onseek={canPlay ? seek : null}
          onedgemove={moveTo}
          onedgeend={endDrag}
          onframeresize={resizeZoom}
          onframepan={panZoom}
          onframefit={fitFrame}
        />
        <CueDetail
          {detail}
          detailStep={DETAIL_STEP}
          {peaks}
          {fileDuration}
          {frame}
          {region}
          markers={detailMarkers}
          {envelope}
          {playhead}
          {selected}
          onseek={canPlay ? seek : null}
          onselect={select}
          onmarkermove={moveTo}
          onmarkerend={endDrag}
        />

        <div class="cue-transport">
          {#if canPlay}
            <button
              id="btn-cue-points-play"
              class="btn btn-icon"
              class:active={mode === "raw"}
              onclick={playRaw}
              title="Play the whole file, ignoring the markers (Space)"
            >
              <span class="material-symbols-outlined" aria-hidden="true"
                >{mode === "raw" && app.cueIsPlaying
                  ? "pause"
                  : "play_arrow"}</span
              >
              {mode === "raw" && app.cueIsPlaying ? "Pause" : "Play"}
            </button>
            <button
              id="btn-cue-points-audition"
              class="btn btn-icon"
              class:active={mode === "audition"}
              onclick={audition}
              title="Hear this edit — trims and fades applied (A)"
            >
              <span class="material-symbols-outlined" aria-hidden="true"
                >{auditionCurrent && app.cueIsPlaying
                  ? "pause"
                  : "headphones"}</span
              >
              {auditionCurrent && app.cueIsPlaying ? "Pause" : "Audition"}
            </button>
            <button
              id="btn-cue-points-stop"
              class="btn btn-icon"
              onclick={stop}
              disabled={!mode}
              title="Back to the start"
              aria-label="Stop"
            >
              <span class="material-symbols-outlined" aria-hidden="true"
                >stop</span
              >
            </button>
            <span class="cue-clock" id="cue-points-clock"
              >{playhead == null ? "—:—" : formatClock(playhead)}</span
            >
          {:else}
            <span class="cue-hint"
              >Choose a cue output in Settings to listen while you edit.</span
            >
          {/if}
          <span class="cue-footer-gap"></span>
          <button
            id="btn-cue-points-fit"
            class="btn btn-icon"
            onclick={fitFrame}
            disabled={!manualFrame}
            title="Fit the zoom to the region — the box also fits on a double-click"
          >
            <span class="material-symbols-outlined" aria-hidden="true"
              >fit_screen</span
            >
            Fit
          </button>
          <span class="cue-summary">
            Airs <strong>{formatTime(airSeconds)}</strong> of
            {formatTime(fileDuration)}
          </span>
        </div>

        <p class="cue-notice">
          {#if dirty}
            <span class="cue-dirty"
              >Unsaved. These changes are discarded unless you save them to the
              track or use them for one airing.</span
            >
          {:else}
            Saving applies these to every airing, from this track's next one.
          {/if}
          <span class="cue-keys"
            >Keys: Space play · A audition · I O F G N mark · ←/→ nudge · P
            pre-roll</span
          >
        </p>

        <div class="cue-rows">
          {#each CUE_MARKERS as spec (spec.key)}
            <CueMarkerRow
              {spec}
              value={draft[spec.key]}
              fallback={FALLBACK[spec.key]}
              selected={selected === spec.key}
              onselect={() => select(spec.key)}
              oncommit={(ms) => commitField(spec.key, ms)}
              onnudge={(delta) => nudgeMarker(spec.key, delta)}
              onmark={() => mark(spec.key)}
            />
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
    background: var(--scrim);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .cue-dialog {
    background: var(--surface-container);
    border-radius: var(--r-lg);
    box-shadow: 0 16px 48px
      color-mix(in srgb, var(--shadow-color) 45%, transparent);
    width: min(1200px, calc(100% - 32px));
    max-height: 92vh;
    overflow-y: auto;
  }

  .cue-dialog-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
    padding: 12px 20px;
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
    padding: 14px 20px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .cue-transport {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 4px;
  }

  .cue-clock {
    font-family: var(--font-mono);
    font-size: 13px;
    color: var(--on-surface);
    font-variant-numeric: tabular-nums;
    min-width: 64px;
  }

  .cue-hint,
  .cue-summary,
  .cue-notice {
    font-size: 12px;
    color: var(--on-surface-variant);
  }

  .cue-summary strong {
    color: var(--on-surface);
    font-variant-numeric: tabular-nums;
  }

  .cue-notice {
    margin: 0;
    display: flex;
    flex-wrap: wrap;
    justify-content: space-between;
    gap: 4px 16px;
  }

  .cue-keys {
    opacity: 0.8;
  }

  .cue-dirty {
    color: var(--cue-out-color);
  }

  .cue-rows {
    display: flex;
    flex-direction: column;
    gap: 2px;
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
    padding: 12px 20px;
    border-top: 1px solid
      color-mix(in srgb, var(--outline-variant) 30%, transparent);
  }

  .cue-footer-gap {
    flex: 1;
  }

  .cue-confirm {
    font-size: 13px;
    color: var(--on-surface);
  }

  .btn {
    background: transparent;
    border: 1px solid
      color-mix(in srgb, var(--outline-variant) 40%, transparent);
    color: var(--on-surface-variant);
    border-radius: 4px;
    cursor: pointer;
    font-size: 13px;
    padding: 8px 16px;
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

  .btn.active {
    border-color: color-mix(in srgb, var(--secondary) 60%, transparent);
    color: var(--on-surface);
  }

  .btn:hover:not(:disabled) {
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
</style>
