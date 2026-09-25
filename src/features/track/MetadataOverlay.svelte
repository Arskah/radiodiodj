<script lang="ts">
  import { app } from "../../shared/state.svelte";
  import { EditedField } from "../../shared/types";

  const MIN_YEAR = 1900;
  const MAX_YEAR = 2100;

  let overlay: HTMLDivElement | undefined = $state();
  let titleInput: HTMLInputElement | undefined = $state();
  let title = $state("");
  let artist = $state("");
  let album = $state("");
  let genre = $state("");
  // `type=number` bind:value hands us a number (or null when empty), not the
  // string we seed — normalise on read in handleSave.
  let year = $state<string | number | null>("");
  let albumArtist = $state("");
  let initialKey = $state("");
  let comment = $state("");
  let trackNo = $state<string | number | null>("");
  let trackTotal = $state<string | number | null>("");
  let discNo = $state<string | number | null>("");
  let discTotal = $state<string | number | null>("");
  let saving = $state(false);
  let error = $state<string | null>(null);
  let confirmingRevert = $state(false);

  const edited = $derived(app.editingMetadata?.edited_fields ?? 0);

  $effect(() => {
    if (app.editingMetadata) {
      title = app.editingMetadata.title;
      artist = app.editingMetadata.artist;
      album = app.editingMetadata.album;
      genre = app.editingMetadata.genre ?? "";
      year = app.editingMetadata.year ? String(app.editingMetadata.year) : "";
      albumArtist = app.editingMetadata.album_artist ?? "";
      initialKey = app.editingMetadata.initial_key ?? "";
      comment = app.editingMetadata.comment ?? "";
      trackNo = num(app.editingMetadata.track_no);
      trackTotal = num(app.editingMetadata.track_total);
      discNo = num(app.editingMetadata.disc_no);
      discTotal = num(app.editingMetadata.disc_total);
      error = null;
      confirmingRevert = false;
    } else {
      title = "";
      artist = "";
      album = "";
      genre = "";
      year = "";
      albumArtist = "";
      initialKey = "";
      comment = "";
      trackNo = "";
      trackTotal = "";
      discNo = "";
      discTotal = "";
      error = null;
    }
  });

  /** Seed a numeric box: an absent value is an empty box, not a `0`. */
  function num(v: number | null | undefined): string {
    return v == null ? "" : String(v);
  }

  /**
   * Read a numeric box back. Svelte binds `type=number` to a number, or `null`
   * once it has been cleared, so this normalises both plus the string it was
   * seeded with. An empty box clears the column.
   */
  function parsePosition(
    v: string | number | null,
    label: string,
  ): number | null | typeof INVALID {
    const s = v == null ? "" : String(v).trim();
    if (s === "") return null;
    const n = Number(s);
    // `0` is accepted because taggers write it and the scan stores it. Refusing
    // it would block every other edit to such a track behind a complaint about
    // a box the operator never touched.
    if (!Number.isInteger(n) || n < 0 || n > 9999) {
      error = `${label} must be a whole number between 0 and 9999`;
      return INVALID;
    }
    return n;
  }

  const INVALID = Symbol("invalid");

  /** An empty text box clears its column rather than writing `""`. */
  function orNull(v: string): string | null {
    return v.trim() === "" ? null : v;
  }

  async function handleSave(): Promise<void> {
    const track = app.editingMetadata;
    if (!track) return;

    // The editor sends every field, so an empty box now writes an empty value
    // (partial-patch: present key = set). Title backs document.title and the
    // library rows, so reject a blank one rather than persisting "".
    if (title.trim() === "") {
      error = "Title is required";
      return;
    }

    // `type=number` min/max are hints only; validate the entered year here so a
    // bad value is rejected before it reaches the backend.
    // Svelte binds a numeric input to a number (or null when empty), so coerce
    // to a trimmed string before validating — `year.trim()` would throw on the
    // number the binding produces once the field has been edited.
    const yearStr = year == null ? "" : String(year).trim();
    let parsedYear: number | null = null;
    if (yearStr !== "") {
      const n = Number(yearStr);
      if (!Number.isInteger(n) || n < MIN_YEAR || n > MAX_YEAR) {
        error = `Year must be a whole number between ${MIN_YEAR} and ${MAX_YEAR}`;
        return;
      }
      parsedYear = n;
    }

    // Checked in order and stopped at the first bad box: each call overwrites
    // `error`, so evaluating all four would report the last complaint rather
    // than the one nearest the top of the form.
    const trackNoValue = parsePosition(trackNo, "Track number");
    if (trackNoValue === INVALID) return;
    const trackTotalValue = parsePosition(trackTotal, "Track total");
    if (trackTotalValue === INVALID) return;
    const discNoValue = parsePosition(discNo, "Disc number");
    if (discNoValue === INVALID) return;
    const discTotalValue = parsePosition(discTotal, "Disc total");
    if (discTotalValue === INVALID) return;

    saving = true;
    error = null;
    try {
      const updated = await app.updateTrackMetadata(track.id, {
        title,
        artist,
        album,
        genre: genre || null,
        year: parsedYear,
        album_artist: orNull(albumArtist),
        initial_key: orNull(initialKey),
        comment: orNull(comment),
        track_no: trackNoValue,
        track_total: trackTotalValue,
        disc_no: discNoValue,
        disc_total: discTotalValue,
      });
      if (updated) {
        close();
      } else {
        error = "Failed to save changes";
      }
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      saving = false;
    }
  }

  async function handleRevert(): Promise<void> {
    const track = app.editingMetadata;
    if (!track) return;
    saving = true;
    error = null;
    try {
      await app.revertTrackTags(track.id);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      saving = false;
      confirmingRevert = false;
    }
  }

  function handleKeyDown(e: KeyboardEvent): void {
    if (!app.editingMetadata) return;
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    } else if (e.key === "Enter") {
      const target = e.target as HTMLElement;
      // Enter in a field submits; leave button activations to native clicks.
      if (target.tagName === "INPUT" && !saving) {
        e.preventDefault();
        void handleSave();
      }
    }
  }

  function close(): void {
    app.editingMetadata = null;
  }

  $effect(() => {
    if (app.editingMetadata) {
      document.addEventListener("keydown", handleKeyDown);
      // Focus the first field so the dialog is keyboard-usable on open.
      titleInput?.focus();
      return () => document.removeEventListener("keydown", handleKeyDown);
    }
    return;
  });
</script>

{#if app.editingMetadata}
  <div
    class="editor-overlay"
    role="presentation"
    bind:this={overlay}
    onmousedown={(e) => {
      if (!overlay || e.target !== e.currentTarget) return;
      close();
    }}
  >
    <div
      id="metadata-dialog"
      class="editor-content"
      role="dialog"
      aria-modal="true"
      aria-label="Edit track metadata"
      tabindex="-1"
    >
      <div class="editor-header">
        <h2 class="editor-title">Edit Metadata</h2>
        <button
          id="btn-metadata-close"
          class="btn-close"
          onclick={close}
          title="Close (Escape)"
        >
          <span class="material-symbols-outlined">close</span>
        </button>
      </div>
      <div class="editor-body">
        <label class="field">
          <span
            >Title{#if edited & EditedField.title}<em class="edited-mark"
                >edited</em
              >{/if}</span
          >
          <input
            id="metadata-title"
            type="text"
            bind:this={titleInput}
            bind:value={title}
            autocomplete="off"
          />
        </label>
        <label class="field">
          <span
            >Artist{#if edited & EditedField.artist}<em class="edited-mark"
                >edited</em
              >{/if}</span
          >
          <input
            id="metadata-artist"
            type="text"
            bind:value={artist}
            autocomplete="off"
          />
        </label>
        <label class="field">
          <span
            >Album{#if edited & EditedField.album}<em class="edited-mark"
                >edited</em
              >{/if}</span
          >
          <input
            id="metadata-album"
            type="text"
            bind:value={album}
            autocomplete="off"
          />
        </label>
        <label class="field">
          <span
            >Genre{#if edited & EditedField.genre}<em class="edited-mark"
                >edited</em
              >{/if}</span
          >
          <input
            id="metadata-genre"
            type="text"
            bind:value={genre}
            autocomplete="off"
            placeholder="Clear with ⌫ + Save"
          />
        </label>
        <label class="field">
          <span
            >Year{#if edited & EditedField.year}<em class="edited-mark"
                >edited</em
              >{/if}</span
          >
          <input
            id="metadata-year"
            type="number"
            bind:value={year}
            min="1900"
            max="2100"
            inputmode="numeric"
          />
        </label>
        <label class="field">
          <span
            >Album artist{#if edited & EditedField.album_artist}<em
                class="edited-mark">edited</em
              >{/if}</span
          >
          <input
            id="metadata-album-artist"
            type="text"
            bind:value={albumArtist}
            autocomplete="off"
            placeholder="Clear with ⌫ + Save"
          />
        </label>
        <!-- Number and total on one row: they are two halves of one tag frame,
             and writing the number without the total would drop the "/12". -->
        <div class="field">
          <span
            >Track{#if edited & (EditedField.track_no | EditedField.track_total)}<em
                class="edited-mark">edited</em
              >{/if}</span
          >
          <div class="field-pair">
            <input
              id="metadata-track-no"
              type="number"
              aria-label="Track number"
              bind:value={trackNo}
              min="0"
              max="9999"
              inputmode="numeric"
            />
            <span class="field-sep" aria-hidden="true">of</span>
            <input
              id="metadata-track-total"
              type="number"
              aria-label="Tracks on the record"
              bind:value={trackTotal}
              min="0"
              max="9999"
              inputmode="numeric"
            />
          </div>
        </div>
        <div class="field">
          <span
            >Disc{#if edited & (EditedField.disc_no | EditedField.disc_total)}<em
                class="edited-mark">edited</em
              >{/if}</span
          >
          <div class="field-pair">
            <input
              id="metadata-disc-no"
              type="number"
              aria-label="Disc number"
              bind:value={discNo}
              min="0"
              max="9999"
              inputmode="numeric"
            />
            <span class="field-sep" aria-hidden="true">of</span>
            <input
              id="metadata-disc-total"
              type="number"
              aria-label="Discs in the set"
              bind:value={discTotal}
              min="0"
              max="9999"
              inputmode="numeric"
            />
          </div>
        </div>
        <label class="field">
          <span
            >Key{#if edited & EditedField.initial_key}<em class="edited-mark"
                >edited</em
              >{/if}</span
          >
          <input
            id="metadata-key"
            type="text"
            bind:value={initialKey}
            autocomplete="off"
            placeholder="Am, 8A — stored as typed"
          />
        </label>
        <label class="field">
          <span
            >Comment{#if edited & EditedField.comment}<em class="edited-mark"
                >edited</em
              >{/if}</span
          >
          <textarea
            id="metadata-comment"
            rows="2"
            bind:value={comment}
            placeholder="Clear with ⌫ + Save"></textarea>
        </label>
        {#if error}
          <div id="metadata-error" class="editor-error">{error}</div>
        {/if}
      </div>
      <div class="editor-footer">
        {#if confirmingRevert}
          <span class="editor-confirm"
            >Discard the edits and use the file's tags?</span
          >
          <button
            id="btn-metadata-revert-confirm"
            class="btn"
            onclick={handleRevert}
            disabled={saving}>Revert</button
          >
          <button
            class="btn"
            onclick={() => (confirmingRevert = false)}
            disabled={saving}>Keep edits</button
          >
        {:else}
          {#if edited}
            <button
              id="btn-metadata-revert"
              class="btn btn-revert"
              onclick={() => (confirmingRevert = true)}
              disabled={saving}
              title="Discard the edited fields and read them from the file again"
              >Revert to file tags</button
            >
          {/if}
          <button
            id="btn-metadata-save"
            class="btn btn-primary"
            onclick={handleSave}
            disabled={saving}
          >
            {saving ? "Saving…" : "Save"}
          </button>
          <button
            id="btn-metadata-cancel"
            class="btn"
            onclick={close}
            disabled={saving}
          >
            Cancel
          </button>
        {/if}
      </div>
    </div>
  </div>
{/if}

<style lang="css">
  .editor-overlay {
    position: fixed;
    inset: 0;
    background: var(--scrim);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
    animation: fadeIn 0.15s ease-out;
  }

  .editor-content {
    background: var(--surface-container);
    border-radius: var(--r-xl);
    box-shadow: 0 16px 48px
      color-mix(in srgb, var(--shadow-color) 40%, transparent);
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
    border-bottom: 1px solid var(--outline-variant);
  }

  .editor-title {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
    color: var(--on-surface);
  }

  .btn-close {
    background: none;
    border: none;
    color: var(--on-surface-variant);
    cursor: pointer;
    padding: 4px;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: var(--r-lg);
  }

  .btn-close:hover {
    color: var(--on-surface);
    background: color-mix(in srgb, var(--on-surface) 10%, transparent);
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
    color: var(--outline);
    text-transform: uppercase;
  }

  .field input,
  .field textarea {
    background: transparent;
    border: 1px solid var(--outline-variant);
    border-radius: var(--r-lg);
    padding: 8px 10px;
    color: var(--on-surface);
    font-size: 14px;
    outline: none;
    transition: border-color 0.15s;
  }

  .field textarea {
    resize: vertical;
    font-family: inherit;
    min-height: 2.5em;
  }

  /* Number and its total, side by side. The boxes are narrow because they hold
     at most four digits, and the separator carries no width of its own. */
  .field-pair {
    display: flex;
    align-items: center;
    gap: var(--sp-sm);
  }

  .field-pair input {
    width: 5.5em;
    flex: 0 0 auto;
  }

  .field-sep {
    font-size: 12px;
    color: var(--outline);
    text-transform: none;
  }

  .field input:focus,
  .field textarea:focus {
    border-color: var(--primary);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--primary) 15%, transparent);
  }

  .field input::placeholder,
  .field textarea::placeholder {
    color: var(--outline);
  }

  .editor-error {
    color: var(--error);
    font-size: 13px;
    padding: 8px 10px;
    background: color-mix(in srgb, var(--error) 10%, transparent);
    border-radius: var(--r-lg);
  }

  .edited-mark {
    margin-left: 6px;
    font-style: normal;
    text-transform: none;
    color: var(--primary);
  }

  .editor-confirm {
    margin-right: auto;
    font-size: 13px;
    color: var(--on-surface-variant);
  }

  .btn-revert {
    margin-right: auto;
  }

  .editor-footer {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
    padding: 16px 20px;
    border-top: 1px solid var(--outline-variant);
  }

  .btn {
    background: transparent;
    border: 1px solid var(--outline-variant);
    color: var(--on-surface-variant);
    padding: 8px 16px;
    border-radius: var(--r-lg);
    cursor: pointer;
    font-size: 13px;
    transition: all 0.15s;
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
