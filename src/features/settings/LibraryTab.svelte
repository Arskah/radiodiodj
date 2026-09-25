<script lang="ts">
  import { app } from "../../shared/state.svelte";
  import LibraryHealth from "../health/LibraryHealth.svelte";
  import type { ContentType, TuningConfig } from "../../shared/types";

  interface Props {
    /**
     * The overlay's tuning draft. Bindable because the fields here write into
     * it directly; the overlay is what persists it.
     */
    tuning: TuningConfig;
    saveTuning: (e?: Event) => Promise<void>;
  }

  let { tuning = $bindable(), saveTuning }: Props = $props();

  const sections: { type: ContentType; label: string }[] = [
    { type: "music", label: "Music" },
    { type: "commercial", label: "Commercials" },
    { type: "jingle", label: "Jingles" },
  ];

  async function onScan(): Promise<void> {
    await app.scan();
    app.settingsOpen = false;
  }
</script>

<div class="settings-section">
  <h4>Library</h4>
  <p class="settings-section-desc">
    Where your media lives, and what needs attention in it: tracks whose files
    are gone, copies of the same recording, and changes on disk that no scan has
    picked up yet. Nothing here changes an audio file — delete an unwanted copy
    in the file manager, then scan.
  </p>
  <div id="paths-list">
    {#each sections as { type, label } (type)}
      <div class="path-section">
        <div class="path-section-header">
          <span>{label}</span>
          <button
            class="btn-add-section"
            title="Add {label} folder"
            onclick={() => app.addPath(type)}
          >
            <span class="material-symbols-outlined">add_circle</span>
            Add Directory
          </button>
        </div>
        {#if (app.libraryPaths[type] ?? []).length === 0}
          <div class="path-empty">
            No {label.toLowerCase()} directories defined. Click "Add Directory" to
            begin.
          </div>
        {:else}
          {#each app.libraryPaths[type] as p (p)}
            <div class="path-row">
              <span class="material-symbols-outlined">folder</span>
              <span class="path-text">{p}</span>
              <button
                class="btn-remove"
                title="Remove"
                aria-label="Remove directory"
                onclick={() => app.removePath(type, p)}
              >
                <span class="material-symbols-outlined">close</span>
              </button>
            </div>
          {/each}
        {/if}
      </div>
    {/each}
  </div>
  <div class="settings-row">
    <button
      id="btn-scan-now"
      class="btn-scan-now"
      title="Scan all configured paths"
      onclick={onScan}
    >
      <span class="material-symbols-outlined">sync</span>
      Scan Library Now
    </button>
  </div>
  <div class="np-group" class:disabled={!tuning.library.writeTags}>
    <div class="np-group-header">
      <span class="material-symbols-outlined" aria-hidden="true"
        >edit_document</span
      >
      <span class="np-group-title">Write edits to file tags</span>
      <label class="np-toggle" title="Write metadata edits to files">
        <input
          id="setting-write-tags"
          type="checkbox"
          bind:checked={tuning.library.writeTags}
          onchange={saveTuning}
        />
        <span class="np-toggle-track"></span>
      </label>
    </div>
    <p class="settings-section-desc">
      Metadata edits are always kept in the library. With this on, they are also
      written into the audio file, so they travel with it. This modifies files
      on your library paths, network shares included. Failed writes are listed
      under Library health.
    </p>
  </div>
  <LibraryHealth />
</div>
