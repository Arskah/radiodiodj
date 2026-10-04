<script lang="ts">
  import { app, formatTime } from "../../shared/state.svelte";
  import type { SavedEntry, SavedPlaylistSummary } from "../../shared/types";
  import { airDuration } from "../../shared/cuePoints";
  import { sizeLabel } from "../../shared/savedPlaylists";
  import MissingBadge from "../track/MissingBadge.svelte";
  import { gapAt, moveTarget } from "../playlist/playlistDrop";

  const shown = $derived.by<SavedPlaylistSummary[]>(() => {
    const query = app.searchQuery.trim().toLowerCase();
    if (query === "") return app.savedPlaylists;
    return app.savedPlaylists.filter((saved) =>
      saved.name.toLowerCase().includes(query),
    );
  });

  const open = $derived(app.openSaved);
  const openSummary = $derived(
    open ? app.savedPlaylists.find((saved) => saved.id === open.id) : undefined,
  );

  function onRowKeyDown(saved: SavedPlaylistSummary, e: KeyboardEvent): void {
    if (e.target !== e.currentTarget) return;
    if (e.key !== "Enter" && e.key !== " ") return;
    e.preventDefault();
    void app.openSavedPlaylist(saved.id);
  }

  function append(id: number, weave: boolean, e?: MouseEvent): void {
    e?.stopPropagation();
    void app.addSavedToPlaylist(id, weave);
  }

  function rename(saved: { id: number; name: string }, e?: MouseEvent): void {
    e?.stopPropagation();
    app.savedDialog = { kind: "rename", id: saved.id, name: saved.name };
  }

  function remove(saved: { id: number; name: string }, e?: MouseEvent): void {
    e?.stopPropagation();
    app.savedDialog = { kind: "delete", id: saved.id, name: saved.name };
  }

  const title = (entry: SavedEntry): string =>
    entry.track?.title ?? entry.title;
  const artist = (entry: SavedEntry): string =>
    entry.track?.artist ?? entry.artist;
  const duration = (entry: SavedEntry): number =>
    entry.track ? airDuration(entry.track) : entry.duration;

  // ----- Reordering, admin only -----

  let dragFrom = $state(-1);
  let dropTarget = $state(-1);

  function gapUnder(e: DragEvent): number {
    const list = e.currentTarget as HTMLElement;
    const rows = Array.from(list.querySelectorAll(".saved-entry"), (row) =>
      row.getBoundingClientRect(),
    );
    return gapAt(e.clientY, rows);
  }

  function onDragStart(e: DragEvent, i: number): void {
    dragFrom = i;
    // A library row removed mid-drag never fires the `dragend` that clears it.
    app.draggedTrackIds = null;
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", String(i));
    }
  }

  function onDragEnd(): void {
    dragFrom = -1;
    dropTarget = -1;
  }

  function onDragOver(e: DragEvent): void {
    if (dragFrom === -1) return;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
    dropTarget = gapUnder(e);
  }

  function onDrop(e: DragEvent): void {
    if (dragFrom === -1) return;
    e.preventDefault();
    const to = moveTarget(dragFrom, gapUnder(e));
    const from = dragFrom;
    onDragEnd();
    if (to !== null) app.moveSavedEntry(from, to);
  }
</script>

{#if open}
  <div id="saved-open-header">
    <button
      id="btn-saved-back"
      class="btn-add"
      title="Back to saved playlists"
      aria-label="Back to saved playlists"
      onclick={() => app.closeSavedPlaylist()}
    >
      <span class="material-symbols-outlined">arrow_back</span>
    </button>
    <span id="saved-open-name">{open.name}</span>
    <span id="saved-open-size"
      >{sizeLabel(open.entries.length, openSummary?.missing ?? 0)}</span
    >
    <button
      id="btn-saved-add"
      class="btn-filler"
      title="Add every track to the playlist, as written"
      disabled={open.entries.length === 0}
      onclick={() => append(open.id, false)}>Add to playlist</button
    >
    <button
      id="btn-saved-weave"
      class="btn-filler"
      title="Add to the playlist, with jingles and commercials woven in"
      disabled={open.entries.length === 0}
      onclick={() => append(open.id, true)}>+ Jingles &amp; comm</button
    >
    {#if app.isAdmin}
      <button
        class="btn-edit"
        title="Rename"
        aria-label="Rename saved playlist"
        onclick={() => rename(open)}
      >
        <span class="material-symbols-outlined">edit</span>
      </button>
      <button
        class="btn-edit"
        title="Delete"
        aria-label="Delete saved playlist"
        onclick={() => remove(open)}
      >
        <span class="material-symbols-outlined">delete</span>
      </button>
    {/if}
  </div>
  <div id="saved-entries" ondragover={onDragOver} ondrop={onDrop} role="list">
    {#if open.entries.length === 0}
      <div class="empty">
        <span class="empty-icon"
          ><span class="material-symbols-outlined">playlist_add</span></span
        >
        <span class="empty-title">Nothing In This Playlist</span>
        <span class="empty-body"
          >Add tracks from a library row's menu, while admin mode is unlocked.</span
        >
      </div>
    {:else}
      {#each open.entries as entry, i (entry.id)}
        <div
          class="saved-entry"
          class:unmatched={entry.track === null}
          class:dragging={i === dragFrom}
          class:drop-before={dragFrom !== -1 && dropTarget === i}
          class:drop-after={dragFrom !== -1 &&
            dropTarget === i + 1 &&
            i === open.entries.length - 1}
          draggable={app.isAdmin}
          ondragstart={(e) => onDragStart(e, i)}
          ondragend={onDragEnd}
          role="listitem"
          data-entry-id={entry.id}
        >
          <span class="saved-entry-no">{i + 1}</span>
          <span class="saved-entry-title">{title(entry)}</span>
          <span class="saved-entry-artist">{artist(entry)}</span>
          {#if entry.track}
            <MissingBadge trackId={entry.track.id} />
          {:else}
            <span
              class="missing-badge"
              title="No track in this library matches this entry"
              aria-label="Unmatched entry"
              ><span class="material-symbols-outlined">link_off</span></span
            >
          {/if}
          <span class="track-duration">{formatTime(duration(entry))}</span>
          {#if entry.track}
            {@const track = entry.track}
            <button
              class="btn-add"
              title="Add to playlist"
              aria-label="Add to playlist"
              onclick={() => app.addToPlaylist(track)}
            >
              <span class="material-symbols-outlined">add</span>
            </button>
          {/if}
          {#if app.isAdmin}
            <button
              class="btn-edit btn-saved-remove"
              title="Remove from saved playlist"
              aria-label="Remove from saved playlist"
              onclick={() => app.removeSavedEntry(entry.id)}
            >
              <span class="material-symbols-outlined">close</span>
            </button>
          {/if}
        </div>
      {/each}
    {/if}
  </div>
{:else}
  <div id="saved-playlists" role="list">
    {#if app.savedPlaylists.length === 0}
      <div class="empty">
        <span class="empty-icon"
          ><span class="material-symbols-outlined">queue_music</span></span
        >
        <span class="empty-title">No Saved Playlists</span>
        <span class="empty-body"
          >Queue a show, then Save As in the playlist panel to keep it for
          later.</span
        >
      </div>
    {:else}
      {#each shown as saved (saved.id)}
        <div
          class="saved-row"
          onclick={() => void app.openSavedPlaylist(saved.id)}
          onkeydown={(e) => onRowKeyDown(saved, e)}
          role="button"
          tabindex="0"
          aria-label={`Saved playlist: ${saved.name}`}
          data-saved-id={saved.id}
        >
          <span
            class="material-symbols-outlined saved-row-icon"
            aria-hidden="true">queue_music</span
          >
          <span class="saved-row-name">{saved.name}</span>
          <span class="saved-row-size" class:has-missing={saved.missing > 0}
            >{sizeLabel(saved.entries, saved.missing)}</span
          >
          {#if app.isAdmin}
            <button
              class="btn-edit"
              title="Rename"
              aria-label="Rename saved playlist"
              onclick={(e) => rename(saved, e)}
            >
              <span class="material-symbols-outlined">edit</span>
            </button>
            <button
              class="btn-edit"
              title="Delete"
              aria-label="Delete saved playlist"
              onclick={(e) => remove(saved, e)}
            >
              <span class="material-symbols-outlined">delete</span>
            </button>
          {/if}
          <button
            class="btn-add"
            title="Add to playlist"
            aria-label="Add saved playlist to playlist"
            disabled={saved.entries === 0}
            onclick={(e) => append(saved.id, false, e)}
          >
            <span class="material-symbols-outlined">add</span>
          </button>
        </div>
      {/each}
    {/if}
  </div>
{/if}
{#if app.savedNotice}
  <div id="saved-notice" role="status">
    <span>{app.savedNotice}</span>
    <button
      class="btn-edit"
      title="Dismiss"
      aria-label="Dismiss"
      onclick={() => (app.savedNotice = null)}
    >
      <span class="material-symbols-outlined">close</span>
    </button>
  </div>
{/if}

<style>
  #saved-playlists,
  #saved-entries {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: var(--sp-xs) 0;
  }

  .saved-row,
  .saved-entry {
    display: flex;
    align-items: center;
    gap: var(--sp-md);
    padding: 7px var(--sp-md);
    font-size: 13px;
    cursor: default;
    user-select: none;
  }

  .saved-row:nth-child(even),
  .saved-entry:nth-child(even) {
    background: color-mix(
      in srgb,
      var(--surface-container-lowest) 30%,
      transparent
    );
  }

  .saved-row:hover,
  .saved-entry:hover {
    background: color-mix(in srgb, var(--surface-variant) 25%, transparent);
  }

  .saved-row-icon {
    font-size: 18px;
    color: var(--outline);
  }

  .saved-row-name,
  .saved-entry-title {
    flex: 2;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--on-surface);
  }

  .saved-entry-artist {
    flex: 1.5;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--on-surface-variant);
  }

  .saved-row-size,
  #saved-open-size,
  .saved-entry-no {
    color: var(--on-surface-variant);
    font-family: var(--font-mono);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  .saved-row-size.has-missing {
    color: var(--error);
  }

  .saved-entry-no {
    width: 24px;
    text-align: right;
  }

  .saved-entry.unmatched .saved-entry-title,
  .saved-entry.unmatched .saved-entry-artist {
    color: var(--outline);
    font-style: italic;
  }

  .saved-entry.dragging {
    opacity: 0.4;
  }

  .saved-entry.drop-before {
    box-shadow: inset 0 2px 0 0 var(--primary);
  }

  .saved-entry.drop-after {
    box-shadow: inset 0 -2px 0 0 var(--primary);
  }

  #saved-open-header {
    display: flex;
    align-items: center;
    gap: var(--sp-sm);
    padding: var(--sp-xs) var(--sp-md);
    border-bottom: 1px solid var(--outline-variant);
  }

  #saved-open-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13px;
    font-weight: 600;
    color: var(--on-surface);
  }

  .btn-add:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  #saved-notice {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-md);
    padding: var(--sp-xs) var(--sp-md);
    border-top: 1px solid var(--outline-variant);
    font-size: 12px;
    color: var(--on-surface-variant);
  }
</style>
