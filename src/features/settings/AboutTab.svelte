<script lang="ts">
  import { app } from "../../shared/state.svelte";
  import { APP_NAME } from "../../shared/appName";
  import { api } from "../../shared/api";
  import { plainNotes } from "../../shared/releaseNotes";
  import type { ProjectLink, TuningConfig } from "../../shared/types";

  interface Props {
    /** The overlay's tuning draft; the automatic-check switch writes into it. */
    tuning: TuningConfig;
    saveTuning: (e?: Event) => Promise<void>;
  }

  let { tuning = $bindable(), saveTuning }: Props = $props();

  const MIB = 1024 * 1024;

  const phase = $derived(app.update.phase);
  const offer = $derived(app.update.offer);
  const working = $derived(
    phase.kind === "checking" ||
      phase.kind === "downloading" ||
      phase.kind === "installing",
  );

  // Installing restarts the app, which stops whatever is on air. A second
  // press is the confirmation; anything else that happens withdraws it.
  let confirming = $state(false);
  $effect(() => {
    if (!app.settingsOpen || !app.isPlaying || phase.kind !== "available") {
      confirming = false;
    }
  });

  const status = $derived.by(() => {
    switch (phase.kind) {
      case "idle":
        return "Not checked yet.";
      case "checking":
        return "Checking…";
      case "upToDate":
        return "This is the latest version.";
      case "available":
        return `Version ${offer?.version ?? ""} is available.`;
      case "downloading": {
        const done = (phase.done / MIB).toFixed(1);
        return phase.total
          ? `Downloading… ${done} of ${(phase.total / MIB).toFixed(1)} MB`
          : `Downloading… ${done} MB`;
      }
      case "installing":
        return "Installing — the app will restart.";
      case "failed":
        return `Failed: ${phase.message}`;
    }
  });

  function install(): void {
    if (app.isPlaying && !confirming) {
      confirming = true;
      return;
    }
    confirming = false;
    void app.installUpdate();
  }

  function open(link: ProjectLink): void {
    void api.openLink(link);
  }
</script>

<div class="settings-section">
  <h4>About</h4>
  <div class="about-identity">
    <span class="np-group-title">{APP_NAME}</span>
    <span id="about-version" class="about-version"
      >v{app.update.currentVersion}</span
    >
  </div>

  <div class="np-group">
    <div class="np-group-header">
      <span class="material-symbols-outlined" aria-hidden="true"
        >system_update_alt</span
      >
      <span class="np-group-title">Updates</span>
    </div>
    <p
      id="about-update-status"
      class="about-status"
      class:about-status--failed={phase.kind === "failed"}
      role="status"
    >
      {status}
    </p>

    {#if offer && phase.kind !== "installing"}
      {#if offer.notes}
        <pre class="about-notes">{plainNotes(offer.notes)}</pre>
      {/if}
      {#if !offer.installable}
        <p class="settings-section-desc">
          This installation cannot update itself. Download the new version from
          radiodiodj.org and install it the way this one was installed.
        </p>
      {:else if confirming}
        <p class="settings-section-desc" role="alert">
          Something is on air. Restarting stops playback until the app is back —
          a few seconds of silence.
        </p>
      {/if}
    {/if}

    <div class="np-action-row">
      {#if offer?.installable}
        <button
          id="btn-update-install"
          class="btn-scan-now"
          disabled={working}
          onclick={install}
        >
          {confirming
            ? "Restart now — this stops playback"
            : "Download and restart"}
        </button>
      {:else if offer}
        <button
          id="btn-update-website"
          class="btn-scan-now"
          onclick={() => open("website")}
        >
          Get it from radiodiodj.org
        </button>
      {/if}
      <button
        id="btn-update-check"
        class="np-browse"
        disabled={working}
        onclick={() => app.checkForUpdate()}
      >
        Check for updates
      </button>
      {#if confirming}
        <button class="np-browse" onclick={() => (confirming = false)}
          >Cancel</button
        >
      {/if}
    </div>

    <div class="np-subsetting">
      <div class="np-group-header">
        <span class="np-group-title">Check automatically</span>
        <label class="np-toggle" title="Check for updates automatically">
          <input
            id="setting-update-auto-check"
            type="checkbox"
            bind:checked={tuning.updates.autoCheck}
            onchange={saveTuning}
          />
          <span class="np-toggle-track"></span>
        </label>
      </div>
      <p class="settings-section-desc">
        Asks radiodiodj.org for a newer version shortly after launch and a few
        times a day. Nothing is downloaded or installed until you press the
        button, and the app never restarts on its own.
      </p>
    </div>
  </div>

  <div class="about-links">
    <button class="np-browse" onclick={() => open("website")}>Website</button>
    <button class="np-browse" onclick={() => open("changelog")}
      >Changelog</button
    >
    <button class="np-browse" onclick={() => open("source")}>Source code</button
    >
    <button class="np-browse" onclick={() => open("licence")}
      >Licence (GPL-3.0-or-later)</button
    >
  </div>
</div>
