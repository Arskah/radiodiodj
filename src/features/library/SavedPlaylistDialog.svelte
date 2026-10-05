<script lang="ts">
  import { app } from "../../shared/state.svelte";

  let nameInput: HTMLInputElement | undefined = $state();
  let name = $state("");
  let busy = $state(false);
  let error = $state<string | null>(null);

  const dialog = $derived(app.savedDialog);

  const heading = $derived.by(() => {
    switch (dialog?.kind) {
      case "saveAs":
        return "Save playlist as";
      case "addTo":
        return "Add to saved playlist";
      case "rename":
        return "Rename saved playlist";
      case "delete":
        return "Delete saved playlist";
      default:
        return "";
    }
  });

  const confirmLabel = $derived.by(() => {
    switch (dialog?.kind) {
      case "addTo":
        return "Create";
      case "rename":
        return "Rename";
      case "delete":
        return "Delete";
      default:
        return "Save";
    }
  });

  $effect(() => {
    if (!dialog) return;
    name = dialog.kind === "rename" ? dialog.name : "";
    error = null;
    busy = false;
    nameInput?.focus();
    nameInput?.select();
  });

  function close(): void {
    app.savedDialog = null;
  }

  async function run(action: () => Promise<void>): Promise<void> {
    if (busy) return;
    busy = true;
    error = null;
    try {
      await action();
      close();
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      busy = false;
    }
  }

  function submit(e: SubmitEvent): void {
    e.preventDefault();
    const current = dialog;
    if (!current) return;
    void run(() => {
      switch (current.kind) {
        case "saveAs":
          return app.savePlaylistAs(name);
        case "addTo":
          return app.createSavedPlaylist(name, current.trackIds);
        case "rename":
          return app.renameSavedPlaylist(current.id, name);
        case "delete":
          return app.deleteSavedPlaylist(current.id);
      }
    });
  }

  function addTo(id: number): void {
    const current = dialog;
    if (current?.kind !== "addTo") return;
    void run(() => app.addToSavedPlaylist(id, current.trackIds));
  }

  function onKeyDown(e: KeyboardEvent): void {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    }
  }
</script>

{#if dialog}
  <div
    class="dialog-scrim saved-overlay"
    role="presentation"
    onmousedown={(e) => {
      if (e.target === e.currentTarget) close();
    }}
  >
    <div
      id="saved-dialog"
      class="dialog-card saved-content"
      role="dialog"
      aria-modal="true"
      aria-label={heading}
      tabindex="-1"
      onkeydown={onKeyDown}
    >
      <form class="saved-form" onsubmit={submit}>
        <h2 class="saved-title">{heading}</h2>
        {#if dialog.kind === "delete"}
          <p class="saved-desc">
            Delete “{dialog.name}”? Its tracks stay in the library.
          </p>
        {:else}
          {#if dialog.kind === "addTo" && app.savedPlaylists.length > 0}
            <div class="saved-choices" role="list">
              {#each app.savedPlaylists as saved (saved.id)}
                <button
                  type="button"
                  class="saved-choice"
                  disabled={busy}
                  onclick={() => addTo(saved.id)}
                >
                  {saved.name}
                </button>
              {/each}
            </div>
            <p class="saved-desc">Or make a new one:</p>
          {/if}
          <input
            id="saved-name"
            type="text"
            autocomplete="off"
            aria-label="Saved playlist name"
            placeholder="Name"
            bind:this={nameInput}
            bind:value={name}
          />
        {/if}
        {#if error}
          <div id="saved-error" class="saved-error" role="alert">{error}</div>
        {/if}
        <div class="saved-footer">
          <button type="button" class="btn" onclick={close}>Cancel</button>
          <button
            id="btn-saved-confirm"
            type="submit"
            class="btn btn-primary"
            class:btn-danger={dialog.kind === "delete"}
            disabled={busy || (dialog.kind !== "delete" && name.trim() === "")}
            >{confirmLabel}</button
          >
        </div>
      </form>
    </div>
  </div>
{/if}

<style>
  .saved-overlay {
    --dialog-width: 360px;
  }

  .saved-content {
    padding: 20px;
  }

  .saved-form {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .saved-title {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
    color: var(--on-surface);
  }

  .saved-desc {
    margin: 0;
    font-size: 13px;
    color: var(--on-surface-variant);
  }

  .saved-choices {
    display: flex;
    flex-direction: column;
    gap: 4px;
    max-height: 200px;
    overflow-y: auto;
  }

  .saved-choice {
    text-align: left;
    background: transparent;
    border: 1px solid var(--outline-variant);
    border-radius: var(--r-lg);
    padding: 8px 10px;
    color: var(--on-surface);
    font-size: 13px;
    cursor: pointer;
  }

  .saved-choice:hover:not(:disabled) {
    background: color-mix(in srgb, var(--on-surface) 8%, transparent);
  }

  input {
    background: transparent;
    border: 1px solid var(--outline-variant);
    border-radius: var(--r-lg);
    padding: 8px 10px;
    color: var(--on-surface);
    font-size: 14px;
    outline: none;
  }

  input:focus {
    border-color: var(--primary);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--primary) 15%, transparent);
  }

  .saved-error {
    color: var(--error);
    font-size: 13px;
    padding: 8px 10px;
    background: color-mix(in srgb, var(--error) 10%, transparent);
    border-radius: var(--r-lg);
  }

  .saved-footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  .btn {
    background: transparent;
    border: 1px solid var(--outline-variant);
    color: var(--on-surface-variant);
    padding: 8px 16px;
    border-radius: var(--r-lg);
    cursor: pointer;
    font-size: 13px;
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
    background: var(--primary-container);
    border-color: var(--primary-container);
    color: var(--on-primary-container);
  }

  .btn-primary:hover:not(:disabled) {
    background: color-mix(
      in srgb,
      var(--primary-container) 88%,
      var(--on-primary-container)
    );
  }

  .btn-primary.btn-danger {
    background: color-mix(in srgb, var(--error) 15%, transparent);
    border-color: color-mix(in srgb, var(--error) 40%, transparent);
    color: var(--error);
  }

  .btn-primary.btn-danger:hover:not(:disabled) {
    background: color-mix(in srgb, var(--error) 25%, transparent);
  }
</style>
