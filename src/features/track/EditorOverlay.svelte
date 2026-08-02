<script lang="ts">
  import { app } from "../../shared/state.svelte";

  let overlay: HTMLDivElement | undefined = $state();
  let title = $state("");
  let artist = $state("");
  let album = $state("");
  let genre = $state("");
  let year = $state("");
  let saving = $state(false);
  let error = $state<string | null>(null);

  $effect(() => {
    if (app.editingTrack) {
      title = app.editingTrack.title;
      artist = app.editingTrack.artist;
      album = app.editingTrack.album;
      genre = app.editingTrack.genre ?? "";
      year = app.editingTrack.year ? String(app.editingTrack.year) : "";
      error = null;
    } else {
      title = "";
      artist = "";
      album = "";
      genre = "";
      year = "";
      error = null;
    }
  });

  async function handleSave(): Promise<void> {
    const track = app.editingTrack;
    if (!track) return;
    saving = true;
    error = null;
    try {
      const updated = await app.updateTrackMetadata(track.id, {
        title,
        artist,
        album,
        genre: genre || null,
        year: year ? Number(year) : null,
      });
      if (updated) {
        title = updated.title;
        artist = updated.artist;
        album = updated.album;
        genre = updated.genre ?? "";
        year = updated.year ? String(updated.year) : "";
      } else {
        error = "Failed to save changes";
      }
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      saving = false;
    }
  }

  function handleKeyDown(e: KeyboardEvent): void {
    if ((e.key === "Escape" || e.key === "Enter") && app.editingTrack) {
      const target = e.target as HTMLInputElement;
      if (target.tagName === "INPUT" && !target.disabled) return;
    }
    if (e.key === "Escape") close();
  }

  function close(): void {
    app.editingTrack = null;
  }

  $effect(() => {
    if (app.editingTrack) {
      document.addEventListener("keydown", handleKeyDown);
      return () => document.removeEventListener("keydown", handleKeyDown);
    }
    return;
  });
</script>

{#if app.editingTrack}
  <div
    class="editor-overlay"
    bind:this={overlay}
    onmousedown={(e) => {
      if (!overlay || e.target !== e.currentTarget) return;
      close();
    }}
  >
    <div class="editor-content" role="dialog" aria-label="Edit track metadata">
      <div class="editor-header">
        <h2 class="editor-title">Edit Metadata</h2>
        <button class="btn-close" onclick={close} title="Close (Escape)">
          <span class="material-symbols-outlined">close</span>
        </button>
      </div>
      <div class="editor-body">
        <label class="field">
          <span>Title</span>
          <input type="text" bind:value={title} autocomplete="off" />
        </label>
        <label class="field">
          <span>Artist</span>
          <input type="text" bind:value={artist} autocomplete="off" />
        </label>
        <label class="field">
          <span>Album</span>
          <input type="text" bind:value={album} autocomplete="off" />
        </label>
        <label class="field">
          <span>Genre</span>
          <input
            type="text"
            bind:value={genre}
            autocomplete="off"
            placeholder="Clear with ⌫ + Save"
          />
        </label>
        <label class="field">
          <span>Year</span>
          <input
            type="number"
            bind:value={year}
            min="1900"
            max="2100"
            inputmode="numeric"
          />
        </label>
        {#if error}
          <div class="editor-error">{error}</div>
        {/if}
      </div>
      <div class="editor-footer">
        <button class="btn btn-primary" onclick={handleSave} disabled={saving}>
          {saving ? "Saving…" : "Save"}
        </button>
        <button class="btn" onclick={close} disabled={saving}>Cancel</button>
      </div>
    </div>
  </div>
{/if}

<style lang="css">
  .editor-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
    animation: fadeIn 0.15s ease-out;
  }

  .editor-content {
    background: var(--panel-bg, #1e2430);
    border-radius: 8px;
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.4);
    max-width: 520px;
    width: calc(100% - 32px);
    max-height: 90vh;
    overflow-y: auto;
    animation: slideUp 0.2s ease-out;
  }

  .editor-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid rgba(255, 255, 255, 0.08);
  }

  .editor-title {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
    color: #fff;
  }

  .btn-close {
    background: none;
    border: none;
    color: #888;
    cursor: pointer;
    padding: 4px;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: 4px;
  }

  .btn-close:hover {
    color: #fff;
    background: rgba(255, 255, 255, 0.1);
  }

  .editor-body {
    padding: 20px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .field span {
    font-size: 12px;
    font-weight: 500;
    color: #888;
    text-transform: uppercase;
  }

  .field input {
    background: transparent;
    border: 1px solid rgba(255, 255, 255, 0.15);
    border-radius: 4px;
    padding: 8px 10px;
    color: #fff;
    font-size: 14px;
    outline: none;
    transition: border-color 0.15s;
  }

  .field input:focus {
    border-color: rgba(92, 130, 245, 0.8);
    box-shadow: 0 0 0 2px rgba(92, 130, 245, 0.15);
  }

  .field input::placeholder {
    color: #555;
  }

  .editor-error {
    color: #e74c3c;
    font-size: 13px;
    padding: 8px 10px;
    background: rgba(231, 76, 60, 0.1);
    border-radius: 4px;
  }

  .editor-footer {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
    padding: 16px 20px;
    border-top: 1px solid rgba(255, 255, 255, 0.08);
  }

  .btn {
    background: transparent;
    border: 1px solid rgba(255, 255, 255, 0.15);
    color: #ccc;
    padding: 8px 16px;
    border-radius: 4px;
    cursor: pointer;
    font-size: 13px;
    transition: all 0.15s;
  }

  .btn:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.08);
    color: #fff;
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-primary {
    background: #4e7af5;
    border-color: #4e7af5;
    color: #fff;
  }

  .btn-primary:hover:not(:disabled) {
    background: #5d86f7;
  }

  @keyframes fadeIn {
    from {
      opacity: 0;
    }
    to {
      opacity: 1;
    }
  }

  @keyframes slideUp {
    from {
      transform: translateY(20px);
      opacity: 0;
    }
    to {
      transform: translateY(0);
      opacity: 1;
    }
  }
</style>
