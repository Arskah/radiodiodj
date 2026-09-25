<script lang="ts">
  import { app } from "../../shared/state.svelte";
  import type { TuningConfig } from "../../shared/types";
  import { listInput, numInput, rememberField } from "./numericInput";

  interface Props {
    /**
     * The overlay's tuning draft. Bindable because the fields here write into
     * it directly; the overlay is what persists it.
     */
    tuning: TuningConfig;
    saveTuning: (e?: Event) => Promise<void>;
    /** Resolves once any in-flight tuning save has landed. */
    flushTuning: () => Promise<void>;
  }

  let { tuning = $bindable(), saveTuning, flushTuning }: Props = $props();

  const MIB = 1024 * 1024;

  let recalcResult = $state<string | null>(null);
  let recalculating = $state(false);
  // A scan rewrites the rows a recalculation reads, so the backend refuses
  // one while it runs. Say so before the press rather than after.
  const scanning = $derived(app.scanStatus.status === "running");

  let newPassword = $state("");
  let confirmPassword = $state("");
  let passwordFormOpen = $state(false);
  let confirmingRemove = $state(false);
  let passwordBusy = $state(false);
  let passwordError = $state<string | null>(null);

  $effect(() => {
    if (app.settingsOpen) {
      newPassword = "";
      confirmPassword = "";
      passwordFormOpen = false;
      confirmingRemove = false;
      passwordError = null;
      recalcResult = null;
    }
  });

  /**
   * Edit one of the automatic-cue thresholds. The result line under the button
   * names the levels the last recalculation ran at, so a fresh edit makes it a
   * lie — drop it on the first keystroke.
   */
  function thresholdInput(e: Event, apply: (v: number) => void): void {
    recalcResult = null;
    numInput(e, apply);
  }

  /**
   * Apply the thresholds above to material already analysed. Says what it did
   * rather than how far it got: the rows it re-derived are done when this
   * resolves, and the rows it queued are the analysis bar's business.
   *
   * The levels come back from `app.tuning` rather than the inputs, because the
   * backend rounds and clamps them — the line has to name what was applied, not
   * what was typed.
   */
  async function recalculateAutoCue(): Promise<void> {
    recalculating = true;
    recalcResult = null;
    try {
      // Clicking the button is what blurs a threshold input, so the save and
      // the recalculation are two handlers in one gesture, and the backend
      // reads the thresholds it was given.
      await flushTuning();
      const done = await app.recalculateAutoCue();
      const { silenceDbfs, segueDbfs } = app.tuning.autoCue;
      const at = `at ${silenceDbfs} / ${segueDbfs} dBFS`;
      const parts = [`${done.updated.toLocaleString()} re-derived ${at}`];
      if (done.queued > 0) {
        parts.push(`${done.queued.toLocaleString()} queued for analysis`);
      }
      if (done.manual > 0) {
        parts.push(`${done.manual.toLocaleString()} radio edits left alone`);
      }
      recalcResult = parts.join(" · ");
    } catch (err) {
      recalcResult = err instanceof Error ? err.message : String(err);
    } finally {
      recalculating = false;
    }
  }

  async function savePassword(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    if (newPassword === "") {
      passwordError = "Enter a password";
      return;
    }
    if (newPassword !== confirmPassword) {
      passwordError = "The passwords do not match";
      return;
    }
    passwordBusy = true;
    passwordError = null;
    try {
      await app.setAdminPassword(newPassword);
      newPassword = "";
      confirmPassword = "";
      passwordFormOpen = false;
    } catch (err) {
      passwordError = err instanceof Error ? err.message : String(err);
    } finally {
      passwordBusy = false;
    }
  }

  async function removePassword(): Promise<void> {
    passwordBusy = true;
    passwordError = null;
    try {
      await app.clearAdminPassword();
      confirmingRemove = false;
    } catch (err) {
      passwordError = err instanceof Error ? err.message : String(err);
    } finally {
      passwordBusy = false;
    }
  }

  function saveIdleLockMin(e: Event): void {
    const v = Number((e.currentTarget as HTMLInputElement).value);
    if (!Number.isFinite(v)) return;
    void app.setIdleLockMin(Math.round(v)).catch((err) => {
      passwordError = err instanceof Error ? err.message : String(err);
    });
  }
</script>

<div id="admin-mode-section" class="settings-section settings-section--tuning">
  <h4>Admin Mode</h4>
  <p class="settings-section-desc">
    With a password set, settings, scanning, metadata edits and saving cue
    points to a track need it. This guards against mistakes; it is not a
    security boundary. Forgot it? Quit, delete
    <code>admin.passwordHash</code> from <code>config.json</code>, and relaunch.
  </p>
  {#if passwordFormOpen || !app.admin.passwordSet}
    <form onsubmit={savePassword}>
      <div class="device-row">
        <label for="admin-new-password"
          >{app.admin.passwordSet ? "New password" : "Password"}</label
        >
        <input
          id="admin-new-password"
          type="password"
          autocomplete="new-password"
          bind:value={newPassword}
        />
      </div>
      <div class="device-row">
        <label for="admin-confirm-password">Confirm password</label>
        <input
          id="admin-confirm-password"
          type="password"
          autocomplete="new-password"
          bind:value={confirmPassword}
        />
      </div>
      <div class="admin-actions">
        <button
          id="btn-admin-save-password"
          type="submit"
          class="btn-scan-now"
          disabled={passwordBusy}
          >{app.admin.passwordSet ? "Change password" : "Set password"}</button
        >
        {#if passwordFormOpen}
          <button
            type="button"
            class="np-browse"
            onclick={() => (passwordFormOpen = false)}>Cancel</button
          >
        {/if}
      </div>
    </form>
  {:else}
    <div class="admin-actions">
      {#if confirmingRemove}
        <span>Remove the password? Anyone can then change settings.</span>
        <button
          id="btn-admin-remove-confirm"
          class="btn-scan-now"
          disabled={passwordBusy}
          onclick={removePassword}>Remove</button
        >
        <button class="np-browse" onclick={() => (confirmingRemove = false)}
          >Keep</button
        >
      {:else}
        <button
          id="btn-admin-change-password"
          class="btn-scan-now"
          onclick={() => (passwordFormOpen = true)}>Change password</button
        >
        <button
          id="btn-admin-remove-password"
          class="np-browse"
          onclick={() => (confirmingRemove = true)}>Remove password</button
        >
      {/if}
    </div>
  {/if}
  {#if passwordError}
    <div id="admin-password-error" class="admin-error" role="alert">
      {passwordError}
    </div>
  {/if}
  <div class="device-row">
    <label for="admin-idle-lock">Lock after idle (minutes)</label>
    <input
      id="admin-idle-lock"
      type="number"
      min="1"
      max="240"
      value={app.admin.idleLockMin}
      onchange={saveIdleLockMin}
    />
    <div class="hint">
      Admin mode locks again after this long without input, and on every launch.
    </div>
  </div>
</div>
<div
  class="settings-section settings-section--tuning"
  onfocusin={rememberField}
>
  <h4>Advanced Tuning</h4>
  <p class="settings-section-desc">
    Fine-tune library checks, cue analysis, fades, buffering and network
    resilience. Out-of-range values are clamped on save.
  </p>
  <h5 class="tuning-group-title">Library</h5>
  <div class="device-row">
    <label for="tune-check-interval">Library check interval (minutes)</label>
    <input
      id="tune-check-interval"
      type="number"
      min="0"
      value={tuning.library.checkIntervalMin}
      oninput={(e) => numInput(e, (v) => (tuning.library.checkIntervalMin = v))}
      onchange={saveTuning}
    />
    <div class="hint">
      How often to look for files added, changed or removed since the last scan.
      Reads no audio, and never changes the library. 0 turns it off.
    </div>
  </div>
  <div class="device-row">
    <label for="tune-tag-write-timeout">Tag write timeout (seconds)</label>
    <input
      id="tune-tag-write-timeout"
      type="number"
      min="5"
      max="300"
      value={tuning.library.tagWriteTimeoutSec}
      oninput={(e) =>
        numInput(e, (v) => (tuning.library.tagWriteTimeoutSec = v))}
      onchange={saveTuning}
    />
    <div class="hint">
      How long writing an edit into a file may take before it is reported as
      failed.
    </div>
  </div>
  <h5 class="tuning-group-title">Automatic cue analysis</h5>
  <div class="np-group" class:disabled={!tuning.autoCue.apply}>
    <div class="np-group-header">
      <span class="material-symbols-outlined" aria-hidden="true"
        >content_cut</span
      >
      <span class="np-group-title">Apply automatic cue points</span>
      <label class="np-toggle" title="Apply automatic cue points">
        <input
          id="setting-apply-auto-cue"
          type="checkbox"
          bind:checked={tuning.autoCue.apply}
          onchange={saveTuning}
        />
        <span class="np-toggle-track"></span>
      </label>
    </div>
    <p class="settings-section-desc">
      Trims and handover points derived from the audio are used on air and in
      every duration the app shows. Switched off, every track plays whole — but
      the analysis still runs and keeps its results, so switching back on takes
      effect immediately with no second pass over the library. Radio edits you
      made by hand always apply.
    </p>
    <div class="np-subsetting">
      <div class="np-group-header">
        <span class="np-group-title">Apply automatic Next starts</span>
        <label class="np-toggle" title="Apply automatic Next starts">
          <input
            id="setting-apply-auto-next-start"
            type="checkbox"
            bind:checked={tuning.autoCue.applyNextStart}
            disabled={!tuning.autoCue.apply}
            onchange={saveTuning}
          />
          <span class="np-toggle-track"></span>
        </label>
      </div>
      <p class="settings-section-desc">
        Music hands over before it has finished, overlapping the incoming item.
        Switched off, a track the analysis owns plays to its Cue out and the
        next one starts clean — the derived trims still apply. Next starts you
        set by hand always apply.
      </p>
    </div>
  </div>
  <div class="device-row">
    <label for="tune-silence-db">Silence threshold (dBFS)</label>
    <input
      id="tune-silence-db"
      type="number"
      min="-100"
      max="-4"
      step="1"
      value={tuning.autoCue.silenceDbfs}
      oninput={(e) =>
        thresholdInput(e, (v) => (tuning.autoCue.silenceDbfs = v))}
      onchange={saveTuning}
    />
    <div class="hint">
      Below this there is no programme audio, so Cue in and Cue out trim it off
      each end of a track. Changing it affects later analyses only — nothing
      already analysed is recalculated.
    </div>
  </div>
  <div class="device-row">
    <label for="tune-segue-db">Segue threshold (dBFS)</label>
    <input
      id="tune-segue-db"
      type="number"
      min="-99"
      max="-3"
      step="1"
      value={tuning.autoCue.segueDbfs}
      oninput={(e) => thresholdInput(e, (v) => (tuning.autoCue.segueDbfs = v))}
      onchange={saveTuning}
    />
    <div class="hint">
      How quiet a music track has to get before the next item may start. Always
      kept above the silence threshold. Music only — commercials and jingles get
      no automatic Next start. Still derived and stored while automatic Next
      starts are switched off, so turning them back on costs no second pass.
    </div>
  </div>
  <div class="np-action-row">
    <button
      class="btn-scan-now"
      onclick={recalculateAutoCue}
      disabled={recalculating || scanning}
      title={scanning
        ? "A library scan is running; recalculate when it finishes"
        : "Apply these levels to tracks already analysed"}
      >{recalculating ? "Recalculating…" : "Recalculate now"}</button
    >
    {#if recalcResult}
      <span class="np-test-result">{recalcResult}</span>
    {/if}
  </div>
  <p class="settings-section-desc">
    New levels reach later analyses on their own. This applies them to
    everything already in the library — from each track's stored measurements
    where it has them, and by decoding again where it does not. Radio edits you
    made by hand are left alone.
  </p>
  <h5 class="tuning-group-title">Fades</h5>
  <div class="device-row">
    <label for="tune-fade-out">Fade out (ms)</label>
    <input
      id="tune-fade-out"
      type="number"
      min="200"
      max="30000"
      value={tuning.player.fadeOutMs}
      oninput={(e) => numInput(e, (v) => (tuning.player.fadeOutMs = v))}
      onchange={saveTuning}
    />
    <div class="hint">
      How long the deck's Fade out button takes to reach silence before
      stopping.
    </div>
  </div>
  <div class="device-row">
    <label for="tune-fade-next">Fade to next (ms)</label>
    <input
      id="tune-fade-next"
      type="number"
      min="200"
      max="30000"
      value={tuning.player.fadeToNextMs}
      oninput={(e) => numInput(e, (v) => (tuning.player.fadeToNextMs = v))}
      onchange={saveTuning}
    />
    <div class="hint">
      How long the outgoing track takes to fade under the incoming one. Usually
      shorter than a fade to silence.
    </div>
  </div>
  <h5 class="tuning-group-title">Network &amp; cache</h5>
  <div class="device-row">
    <label for="tune-cache">Prefetch cache size (MiB)</label>
    <input
      id="tune-cache"
      type="number"
      min="16"
      value={Math.round(tuning.cache.maxCacheBytes / MIB)}
      oninput={(e) =>
        numInput(e, (v) => (tuning.cache.maxCacheBytes = Math.round(v * MIB)))}
      onchange={saveTuning}
    />
    <div class="hint">
      RAM budget for prefetched track files. Minimum 16 MiB.
    </div>
  </div>
  <div class="device-row">
    <label for="tune-watchdog">Read watchdog timeout (ms)</label>
    <input
      id="tune-watchdog"
      type="number"
      min="1"
      value={tuning.player.readWatchdogTimeoutMs}
      oninput={(e) =>
        numInput(e, (v) => (tuning.player.readWatchdogTimeoutMs = v))}
      onchange={saveTuning}
    />
    <div class="hint">
      A read stalled longer than this counts as a network hiccup and triggers
      recovery.
    </div>
  </div>
  <div class="device-row">
    <label for="tune-open-retry">Output open-retry interval (ms)</label>
    <input
      id="tune-open-retry"
      type="number"
      min="1"
      value={tuning.player.openRetryIntervalMs}
      oninput={(e) =>
        numInput(e, (v) => (tuning.player.openRetryIntervalMs = v))}
      onchange={saveTuning}
    />
    <div class="hint">
      Wait between attempts to reopen the audio output device.
    </div>
  </div>
  <div class="device-row">
    <label for="tune-read-backoffs">Read retry backoffs (ms)</label>
    <input
      id="tune-read-backoffs"
      type="text"
      value={tuning.player.readRetryBackoffsMs.join(", ")}
      oninput={(e) =>
        listInput(e, (v) => (tuning.player.readRetryBackoffsMs = v))}
      onchange={saveTuning}
    />
    <div class="hint">
      Comma-separated wait times between file-read retries. The last value
      repeats until recovery.
    </div>
  </div>
  <div class="hint">Cache and player settings apply on next restart.</div>
</div>
