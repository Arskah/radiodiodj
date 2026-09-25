<script lang="ts">
  import { app } from "../../shared/state.svelte";
  import type { TuningConfig } from "../../shared/types";
  import { restoreCleared } from "./numericInput";
  import AudioTab from "./AudioTab.svelte";
  import LibraryTab from "./LibraryTab.svelte";
  import PlaylistTab from "./PlaylistTab.svelte";
  import NowPlayingTab from "./NowPlayingTab.svelte";
  import AppearanceTab from "./AppearanceTab.svelte";
  import AdvancedTab from "./AdvancedTab.svelte";

  // Editable draft of the tuning config, shared by every tab that shows a
  // tuning field. Synced from `app.tuning` whenever the overlay opens; each
  // edit persists via `app.saveTuning`, then re-syncs so the backend's clamped
  // values are reflected in the inputs.
  let tuning = $state<TuningConfig>($state.snapshot(app.tuning));
  // The in-flight save, so a handler firing in the same gesture as the blur
  // that started it can wait for the backend to have the new values.
  let tuningSave: Promise<unknown> | null = null;

  $effect(() => {
    if (app.settingsOpen) {
      void app.loadAudioConfig();
      tuning = $state.snapshot(app.tuning);
    }
  });

  async function saveTuning(e?: Event): Promise<void> {
    restoreCleared(e);
    const save = app.saveTuning($state.snapshot(tuning));
    tuningSave = save;
    try {
      await save;
      tuning = $state.snapshot(app.tuning);
    } finally {
      if (tuningSave === save) tuningSave = null;
    }
  }

  async function flushTuning(): Promise<void> {
    await tuningSave;
  }
</script>

<div id="settings-overlay" class:hidden={!app.settingsOpen}>
  <div id="settings-modal">
    <div id="settings-modal-header">
      <span class="settings-modal-title">
        <span class="material-symbols-outlined" aria-hidden="true"
          >settings</span
        >
        Preferences
      </span>
      <button
        id="btn-close-settings-x"
        title="Close"
        aria-label="Close settings"
        onclick={() => (app.settingsOpen = false)}
      >
        <span class="material-symbols-outlined">close</span>
      </button>
    </div>

    <div id="settings-body">
      <div id="settings-sidebar" role="tablist" aria-label="Settings sections">
        <button
          class="settings-tab"
          class:active={app.settingsTab === "audio"}
          role="tab"
          aria-selected={app.settingsTab === "audio"}
          onclick={() => (app.settingsTab = "audio")}
        >
          <span class="material-symbols-outlined">volume_up</span>
          Audio Output
        </button>
        <button
          class="settings-tab"
          class:active={app.settingsTab === "library"}
          role="tab"
          aria-selected={app.settingsTab === "library"}
          onclick={() => (app.settingsTab = "library")}
        >
          <span class="material-symbols-outlined">library_music</span>
          Library
          {#if app.healthAttention > 0}
            <span
              class="attention-badge"
              aria-label={`${app.healthAttention} need attention`}
              >{app.healthAttention}</span
            >
          {/if}
        </button>
        <button
          class="settings-tab"
          class:active={app.settingsTab === "playlist"}
          role="tab"
          aria-selected={app.settingsTab === "playlist"}
          onclick={() => (app.settingsTab = "playlist")}
        >
          <span class="material-symbols-outlined">queue_music</span>
          Playlist
        </button>
        <button
          class="settings-tab"
          class:active={app.settingsTab === "now-playing"}
          role="tab"
          aria-selected={app.settingsTab === "now-playing"}
          onclick={() => (app.settingsTab = "now-playing")}
        >
          <span class="material-symbols-outlined">rss_feed</span>
          Now Playing
        </button>
        <button
          class="settings-tab"
          class:active={app.settingsTab === "appearance"}
          role="tab"
          aria-selected={app.settingsTab === "appearance"}
          onclick={() => (app.settingsTab = "appearance")}
        >
          <span class="material-symbols-outlined">palette</span>
          Appearance
        </button>
        <button
          class="settings-tab"
          class:active={app.settingsTab === "advanced"}
          role="tab"
          aria-selected={app.settingsTab === "advanced"}
          onclick={() => (app.settingsTab = "advanced")}
        >
          <span class="material-symbols-outlined">tune</span>
          Advanced
        </button>
      </div>

      <div id="settings-content">
        {#if app.settingsTab === "audio"}
          <AudioTab bind:tuning {saveTuning} />
        {:else if app.settingsTab === "library"}
          <LibraryTab bind:tuning {saveTuning} />
        {:else if app.settingsTab === "playlist"}
          <PlaylistTab bind:tuning {saveTuning} />
        {:else if app.settingsTab === "now-playing"}
          <NowPlayingTab />
        {:else if app.settingsTab === "appearance"}
          <AppearanceTab />
        {:else if app.settingsTab === "advanced"}
          <AdvancedTab bind:tuning {saveTuning} {flushTuning} />
        {/if}
      </div>
    </div>

    <div id="settings-footer">
      <span class="settings-footer-info">
        <span class="material-symbols-outlined" aria-hidden="true">info</span>
        All changes are saved automatically
      </span>
      <div id="settings-actions">
        <button
          id="btn-close-settings"
          onclick={() => (app.settingsOpen = false)}>Close</button
        >
      </div>
    </div>
  </div>
</div>
