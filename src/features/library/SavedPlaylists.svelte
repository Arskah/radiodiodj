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

  const album = (entry: SavedEntry): string => entry.track?.album ?? "";

  // ----- Dragging -----
  //
  // An entry drags the way a library row does: a bound one can be dropped at a
  // position in the playlist. For an admin the same drag reorders the saved
  // playlist when it is dropped back on it.

  let dragFrom = $state(-1);
  let dropTarget = $state(-1);

  const draggable = (entry: SavedEntry): boolean =>
    entry.track !== null || app.isAdmin;

  function gapUnder(e: DragEvent): number {
    const list = e.currentTarget as HTMLElement;
    const rows = Array.from(list.querySelectorAll(".saved-entry"), (row) =>
      row.getBoundingClientRect(),
    );
    return gapAt(e.clientY, rows);
  }

  function onDragStart(e: DragEvent, entry: SavedEntry, i: number): void {
    dragFrom = app.isAdmin ? i : -1;
    app.draggedTrackIds = entry.track ? [entry.track.id] : null;
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = "copyMove";
      e.dataTransfer.setData(
        "text/plain",
        `${artist(entry)} – ${title(entry)}`,
      );
    }
  }

  function onDragEnd(): void {
    dragFrom = -1;
    dropTarget = -1;
    app.draggedTrackIds = null;
  }

  function onDragLeave(e: DragEvent): void {
    const list = e.currentTarget as HTMLElement;
    if (!(e.relatedTarget instanceof Node) || !list.contains(e.relatedTarget)) {
      dropTarget = -1;
    }
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
    <button
      id="btn-saved-export"
      class="btn-edit"
      title="Export as a file"
      aria-label="Export saved playlist"
      onclick={() => void app.exportSavedPlaylist(open.id, open.name)}
    >
      <span class="material-symbols-outlined">file_export</span>
    </button>
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
  <div class="saved-headers">
    <span class="track-header track-no">#</span>
    <span class="track-header track-title">Title</span>
    <span class="track-header track-artist">Artist</span>
    <span class="track-header track-album">Album</span>
    <span class="track-header track-duration">Time</span>
    <span class="saved-action-space"></span>
    {#if app.isAdmin}<span class="saved-action-space"></span>{/if}
  </div>
  <div
    id="saved-entries"
    ondragover={onDragOver}
    ondragleave={onDragLeave}
    ondrop={onDrop}
    role="list"
  >
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
          class="track-row saved-entry"
          class:unmatched={entry.track === null}
          class:dragging={i === dragFrom}
          class:drop-before={dragFrom !== -1 && dropTarget === i}
          class:drop-after={dragFrom !== -1 &&
            dropTarget === i + 1 &&
            i === open.entries.length - 1}
          draggable={draggable(entry)}
          ondragstart={(e) => onDragStart(e, entry, i)}
          ondragend={onDragEnd}
          role="listitem"
          data-entry-id={entry.id}
        >
          <span class="track-no">{i + 1}</span>
          <span class="track-title saved-entry-title">
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
            {title(entry)}
          </span>
          <span class="track-artist">{artist(entry)}</span>
          <span class="track-album">{album(entry)}</span>
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
          {:else}
            <span class="saved-action-space"></span>
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
  <div id="saved-list-header">
    <button
      id="btn-saved-import"
      class="btn-filler"
      title="Make a saved playlist from a file"
      onclick={() => void app.importSavedPlaylist()}>Import</button
    >
  </div>
  <div id="saved-playlists" role="list">
    {#if app.savedPlaylists.length === 0}
      <div class="empty">
        <span class="empty-icon"
          ><span class="material-symbols-outlined">queue_music</span></span
        >
        <span class="empty-title">No Saved Playlists</span>
        <span class="empty-body"
          >Queue a show, then Save As in the playlist panel to keep it for
          later, or import one from a file.</span
        >
      </div>
    {:else}
      {#each shown as saved (saved.id)}
        <div
          class="track-row saved-row"
          onclick={() => void app.openSavedPlaylist(saved.id)}
          onkeydown={(e) => onRowKeyDown(saved, e)}
          role="button"
          tabindex="0"
          aria-label={`Saved playlist: ${saved.name}`}
          data-saved-id={saved.id}
        >
          <span class="track-no" aria-hidden="true"
            ><span class="material-symbols-outlined saved-row-icon"
              >queue_music</span
            ></span
          >
          <span class="track-title saved-row-name">{saved.name}</span>
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

  /* Rows are the library's `.track-row`, so a saved playlist reads as the same
     list. What is here is only what a saved playlist adds to one. */
  .saved-row {
    cursor: pointer;
  }

  .saved-row-icon {
    font-size: 16px;
    vertical-align: middle;
  }

  .saved-row-size,
  #saved-open-size {
    color: var(--on-surface-variant);
    font-family: var(--font-mono);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  .saved-row-size.has-missing {
    color: var(--error);
  }

  .saved-entry-title :global(.missing-badge) {
    vertical-align: middle;
    margin-right: 2px;
  }

  .saved-entry.unmatched .track-title,
  .saved-entry.unmatched .track-artist {
    color: var(--outline);
    font-style: italic;
    font-weight: 400;
  }

  /* Holds a row button's place, so columns line up down the list whether or
     not a row has that button. */
  .saved-action-space {
    width: 26px;
    flex-shrink: 0;
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

  #saved-list-header {
    display: flex;
    justify-content: flex-end;
    padding: var(--sp-xs) var(--sp-md);
    border-bottom: 1px solid var(--outline-variant);
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
