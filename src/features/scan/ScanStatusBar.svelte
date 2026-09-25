<script lang="ts">
  import { app } from "../../shared/state.svelte";

  let dismissed = $state(false);
  let visible = $state(false);

  $effect(() => {
    const s = app.scanStatus;
    if (s.status === "running") {
      dismissed = false;
      visible = true;
      return;
    }
    if (s.status === "idle" && s.lastResult === null) {
      visible = false;
      return;
    }
    // A finished scan's result stays until the operator dismisses it, so a
    // scan that ends while nobody is looking still reports what it changed.
    visible = !dismissed;
  });

  let pct = $derived.by(() => {
    const s = app.scanStatus;
    if (s.status === "running" && s.total > 0) {
      return Math.min(100, (s.processed / s.total) * 100);
    }
    return 0;
  });

  let label = $derived.by(() => {
    const s = app.scanStatus;
    if (s.status === "running") {
      return s.total > 0
        ? `Scanning library… ${s.processed} / ${s.total}`
        : "Scanning library…";
    }
    if (s.status === "canceled") {
      const detail = s.added > 0 ? `${s.added} new/updated` : "no changes";
      return `Scan canceled at ${s.processed} / ${s.total} — ${detail}`;
    }
    if (s.status === "error") {
      return `Scan failed: ${s.message}`;
    }
    if (s.status === "idle" && s.lastResult) {
      const { total, added, reattached = 0, missing = 0 } = s.lastResult;
      const parts = [
        added > 0 && `${added} new/updated`,
        reattached > 0 && `${reattached} moved`,
        missing > 0 && `${missing} missing`,
      ].filter(Boolean);
      const detail = parts.length > 0 ? parts.join(", ") : "no changes";
      return `Scan complete — ${total} tracks (${detail})`;
    }
    return "";
  });

  // Background waveform pass — its own bar, shown under the tag-scan bar only
  // while it is actively computing.
  let wfVisible = $derived(app.waveformStatus.status === "running");
  let wfPct = $derived.by(() => {
    const s = app.waveformStatus;
    return s.status === "running" && s.total > 0
      ? Math.min(100, (s.processed / s.total) * 100)
      : 0;
  });
  let wfLabel = $derived.by(() => {
    const s = app.waveformStatus;
    return s.status === "running"
      ? `Computing waveforms… ${s.processed} / ${s.total}`
      : "";
  });

  function onCancel(): void {
    void app.cancelScan();
  }

  function onCancelWaveform(): void {
    void app.cancelAnalysis();
  }

  function onDismiss(): void {
    dismissed = true;
    visible = false;
  }
</script>

{#if visible || wfVisible}
  <div id="status-bars">
    {#if visible}
      <div
        id="scan-status-bar"
        class="scan-status-bar"
        role="status"
        aria-live="polite"
        data-state={app.scanStatus.status}
      >
        <span class="scan-status-label">{label}</span>
        {#if app.scanStatus.status === "running"}
          <div class="scan-status-bar-track">
            <div class="scan-status-bar-fill" style:width="{pct}%"></div>
          </div>
          <button
            type="button"
            class="scan-status-cancel"
            onclick={onCancel}
            disabled={!app.isAdmin}
            title={app.isAdmin ? undefined : "Unlock admin mode to cancel"}
          >
            Cancel
          </button>
        {:else}
          <button type="button" class="scan-status-cancel" onclick={onDismiss}>
            Dismiss
          </button>
        {/if}
      </div>
    {/if}
    {#if wfVisible}
      <div
        id="waveform-status-bar"
        class="scan-status-bar"
        role="status"
        aria-live="polite"
      >
        <span class="scan-status-label">{wfLabel}</span>
        <div class="scan-status-bar-track">
          <div class="scan-status-bar-fill" style:width="{wfPct}%"></div>
        </div>
        <button
          type="button"
          class="scan-status-cancel"
          onclick={onCancelWaveform}
          disabled={!app.isAdmin}
          title={app.isAdmin ? undefined : "Unlock admin mode to cancel"}
        >
          Cancel
        </button>
      </div>
    {/if}
  </div>
{/if}
