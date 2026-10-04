<script lang="ts">
  import { app, formatTime } from "../../shared/state.svelte";
  import type { SavedEntry, SavedPlaylistSummary } from "../../shared/types";
  import { airDuration, isTrimmed } from "../../shared/cuePoints";
  import {
    matchesSearch,
    sizeLabel,
    sortEntries,
    type EntrySort,
  } from "../../shared/savedPlaylists";
  import MissingBadge from "../track/MissingBadge.svelte";
  import { gapAt, moveTarget } from "../playlist/playlistDrop";
  import ContextMenu from "../ui/ContextMenu.svelte";
  import type { MenuItem } from "../ui/contextMenu";

  const shown = $derived.by<SavedPlaylistSummary[]>(() => {
    const query = app.searchQuery.trim().toLowerCase();
    if (query === "") return app.savedPlaylists;
    return app.savedPlaylists.filter((saved) =>
      saved.name.toLowerCase().includes(query),
    );
  });

  const open = $derived(app.openSaved);

  // The search box means the list of saved playlists while none is open and
  // the open one's entries once one is, so it starts empty on the way in and
  // on the way out.
  function openSaved(id: number): void {
    app.searchQuery = "";
    sortBy = "position";
    sortDir = "asc";
    void app.openSavedPlaylist(id);
  }

  function back(): void {
    app.searchQuery = "";
    app.closeSavedPlaylist();
  }
  const openSummary = $derived(
    open ? app.savedPlaylists.find((saved) => saved.id === open.id) : undefined,
  );

  function onRowKeyDown(saved: SavedPlaylistSummary, e: KeyboardEvent): void {
    if (e.target !== e.currentTarget) return;
    // The bindings the library rows use; macOS keyboards have no menu key.
    const wantsMenu =
      e.key === "ContextMenu" ||
      (e.key === "F10" && e.shiftKey) ||
      (e.key === "Enter" && e.ctrlKey);
    if (wantsMenu) {
      e.preventDefault();
      showMenu({ kind: "saved", saved }, e.currentTarget as HTMLElement, null);
      return;
    }
    if (e.key !== "Enter" && e.key !== " ") return;
    e.preventDefault();
    openSaved(saved.id);
  }

  // ----- Row menus -----

  type MenuTarget =
    | { kind: "saved"; saved: SavedPlaylistSummary }
    | { kind: "entry"; entry: SavedEntry };

  let menu = $state<{ target: MenuTarget; x: number; y: number } | null>(null);
  // The row the menu was opened from, so focus goes back where it came from.
  let menuRow: HTMLElement | null = null;

  /** `at` is the cursor point; without one the menu hangs off the row itself. */
  function showMenu(
    target: MenuTarget,
    row: HTMLElement,
    at: { x: number; y: number } | null,
  ): void {
    if (itemsFor(target).length === 0) return;
    // The hover tooltip is anchored to the row and would sit under the menu.
    app.clearHover();
    const rect = row.getBoundingClientRect();
    menuRow = row;
    menu = {
      target,
      x: at ? at.x : rect.left,
      y: at ? at.y : rect.bottom,
    };
  }

  function openMenu(target: MenuTarget, e: MouseEvent): void {
    e.preventDefault();
    const keyboard = e.clientX === 0 && e.clientY === 0;
    showMenu(
      target,
      e.currentTarget as HTMLElement,
      keyboard ? null : { x: e.clientX, y: e.clientY },
    );
  }

  function closeMenu(restoreFocus: boolean): void {
    menu = null;
    if (restoreFocus) menuRow?.focus();
    menuRow = null;
  }

  function savedItems(saved: SavedPlaylistSummary): MenuItem[] {
    const isSource = app.autoSource?.id === saved.id;
    const items: MenuItem[] = [
      {
        label: "Open",
        icon: "folder_open",
        onselect: () => openSaved(saved.id),
      },
    ];
    if (saved.entries > 0) {
      items.push(
        {
          label: "Add to playlist",
          icon: "add",
          onselect: () => append(saved.id, false),
        },
        {
          label: "Add with jingles and commercials",
          icon: "playlist_add",
          onselect: () => append(saved.id, true),
        },
      );
    }
    items.push(
      {
        label: isSource ? "Stop using as Auto source" : "Use as Auto source",
        icon: "auto_awesome",
        onselect: () => void app.setAutoSource(isSource ? null : saved.id),
        separated: true,
      },
      {
        label: "Export…",
        icon: "file_export",
        onselect: () => void app.exportSavedPlaylist(saved.id, saved.name),
      },
    );
    if (app.isAdmin) {
      items.push(
        {
          label: "Rename…",
          icon: "edit",
          onselect: () => rename(saved),
          separated: true,
        },
        { label: "Delete…", icon: "delete", onselect: () => remove(saved) },
      );
    }
    return items;
  }

  // Play-now sits last, behind a divider, as it does on a library row: it is
  // never the item under the cursor when the menu opens.
  function entryItems(entry: SavedEntry): MenuItem[] {
    const track = entry.track;
    const items: MenuItem[] = [];
    if (track) {
      items.push(
        {
          label: "Add to playlist",
          icon: "add",
          onselect: () => app.addToPlaylist(track),
        },
        {
          label: "Add as next",
          icon: "playlist_play",
          onselect: () => app.addNextToPlaylist(track),
        },
      );
      if (app.cueDevice !== null) {
        items.push({
          label: "Preview on cue deck",
          icon: "headphones",
          onselect: () => app.cueLoad(track),
        });
      }
      if (app.isAdmin) {
        items.push({
          label: "Edit metadata…",
          icon: "edit",
          onselect: () => (app.editingMetadata = track),
        });
      }
      items.push(
        {
          label: "Cue points…",
          icon: "line_start_diamond",
          onselect: () => (app.editingCuePoints = track),
        },
        {
          label: "Show in folder",
          icon: "folder_open",
          onselect: () => app.revealTrack(track),
          separated: true,
        },
      );
    }
    if (app.isAdmin) {
      items.push({
        label: "Remove from saved playlist",
        icon: "close",
        onselect: () => app.removeSavedEntry(entry.id),
        separated: track !== null,
      });
    }
    if (track) {
      items.push({
        label: "Play now (on air)",
        icon: "play_arrow",
        onselect: () => app.playNow(track),
        separated: true,
        danger: true,
      });
    }
    return items;
  }

  const itemsFor = (target: MenuTarget): MenuItem[] =>
    target.kind === "saved"
      ? savedItems(target.saved)
      : entryItems(target.entry);

  const menuItems = $derived(menu ? itemsFor(menu.target) : []);
  const menuLabel = $derived.by(() => {
    if (!menu) return "";
    return menu.target.kind === "saved"
      ? `Actions for ${menu.target.saved.name}`
      : `Actions for ${title(menu.target.entry)}`;
  });

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

  // ----- What an entry row shares with a library row -----

  function onEnter(entry: SavedEntry, e: MouseEvent): void {
    if (!entry.track) return;
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    app.setHover(entry.track, rect);
  }

  function onEntryDblClick(entry: SavedEntry, e: MouseEvent): void {
    if (!entry.track) return;
    // Two quick presses of a row button are two presses of that button.
    if (e.target instanceof Element && e.target.closest("button")) return;
    e.preventDefault();
    app.addToPlaylist(entry.track);
  }

  function onEntryKeyDown(entry: SavedEntry, e: KeyboardEvent): void {
    if (e.target !== e.currentTarget) return;
    const wantsMenu =
      e.key === "ContextMenu" ||
      (e.key === "F10" && e.shiftKey) ||
      (e.key === "Enter" && e.ctrlKey);
    if (!wantsMenu) return;
    e.preventDefault();
    showMenu({ kind: "entry", entry }, e.currentTarget as HTMLElement, null);
  }

  // ----- Sorting -----
  //
  // A sort is a way of looking at the saved playlist, never a change to it: the
  // order it is stored and queued in is the one under `#`.

  let sortBy = $state<EntrySort>("position");
  let sortDir = $state<"asc" | "desc">("asc");

  const sortCols: { column: EntrySort; label: string; cls: string }[] = [
    { column: "position", label: "#", cls: "track-no" },
    { column: "title", label: "Title", cls: "track-title" },
    { column: "artist", label: "Artist", cls: "track-artist" },
    { column: "album", label: "Album", cls: "track-album" },
    { column: "plays", label: "Plays", cls: "track-plays" },
    { column: "duration", label: "Time", cls: "track-duration" },
  ];

  function toggleSort(column: EntrySort): void {
    if (sortBy === column) {
      sortDir = sortDir === "asc" ? "desc" : "asc";
    } else {
      sortBy = column;
      sortDir = "asc";
    }
  }

  function sortIcon(column: EntrySort): string {
    if (sortBy !== column) return "unfold_more";
    return sortDir === "asc" ? "arrow_upward" : "arrow_downward";
  }

  function ariaSort(column: EntrySort): "ascending" | "descending" | "none" {
    if (sortBy !== column) return "none";
    return sortDir === "asc" ? "ascending" : "descending";
  }

  /** The open saved playlist's entries as shown: searched, then sorted. */
  const listed = $derived(
    sortEntries(
      (open?.entries ?? [])
        .map((entry, position) => ({
          entry,
          position,
          title: title(entry),
          artist: artist(entry),
          album: album(entry),
          plays: entry.track?.play_count ?? 0,
          duration: duration(entry),
        }))
        .filter((row) =>
          matchesSearch(app.searchQuery, [row.title, row.artist, row.album]),
        ),
      sortBy,
      sortDir,
    ),
  );
  /**
   * The rows on screen are the saved playlist's own, in its own order. Only
   * then does a gap between two of them name a position, so only then can a
   * drag reorder.
   */
  const inOwnOrder = $derived(
    app.searchQuery.trim() === "" && sortBy === "position" && sortDir === "asc",
  );

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
    // The tooltip is anchored to the row and would hang over the drag.
    app.clearHover();
    dragFrom = app.isAdmin && inOwnOrder ? i : -1;
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
      onclick={back}
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
      id="btn-saved-source"
      class="btn-filler"
      class:active={app.autoSource?.id === open.id}
      title={app.autoSource?.id === open.id
        ? "Auto Mode draws its music from this saved playlist. Click to go back to the music library."
        : "Have Auto Mode draw its music from this saved playlist"}
      aria-pressed={app.autoSource?.id === open.id}
      onclick={() =>
        void app.setAutoSource(app.autoSource?.id === open.id ? null : open.id)}
      >Auto source</button
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
    {#each sortCols as col (col.column)}
      <button
        class="track-header {col.cls}"
        class:active={sortBy === col.column}
        role="columnheader"
        aria-sort={ariaSort(col.column)}
        onclick={() => toggleSort(col.column)}
      >
        {col.label}
        <span class="material-symbols-outlined" aria-hidden="true"
          >{sortIcon(col.column)}</span
        >
      </button>
    {/each}
    {#if app.cueDevice !== null}<span class="saved-action-space"></span>{/if}
    {#if app.isAdmin}<span class="saved-action-space"></span>{/if}
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
    {:else if listed.length === 0}
      <div class="empty">
        <span class="empty-icon"
          ><span class="material-symbols-outlined">search_off</span></span
        >
        <span class="empty-title">No Match In This Playlist</span>
      </div>
    {:else}
      {#each listed as { entry, position: i } (entry.id)}
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
          oncontextmenu={(e) => openMenu({ kind: "entry", entry }, e)}
          ondblclick={(e) => onEntryDblClick(entry, e)}
          onmouseenter={(e) => onEnter(entry, e)}
          onmouseleave={() => app.clearHover()}
          onkeydown={(e) => onEntryKeyDown(entry, e)}
          role="button"
          aria-label={`Entry ${i + 1}: ${title(entry)} by ${artist(entry)}`}
          aria-haspopup="menu"
          tabindex="0"
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
          <span class="track-plays">{entry.track?.play_count ?? ""}</span>
          <span
            class="track-duration"
            class:trimmed={entry.track !== null && isTrimmed(entry.track)}
            >{formatTime(duration(entry))}</span
          >
          {#if entry.track}
            {@const track = entry.track}
            {#if app.cueDevice !== null}
              <button
                class="btn-cue"
                title="Preview on cue deck"
                aria-label="Cue track"
                onclick={() => app.cueLoad(track)}
              >
                <span class="material-symbols-outlined">headphones</span>
              </button>
            {/if}
            {#if app.isAdmin}
              <button
                class="btn-edit"
                title="Edit metadata"
                aria-label="Edit track metadata"
                onclick={() => (app.editingMetadata = track)}
              >
                <span class="material-symbols-outlined">edit</span>
              </button>
            {/if}
            <button
              class="btn-add"
              title="Add to playlist"
              aria-label="Add to playlist"
              onclick={() => app.addToPlaylist(track)}
            >
              <span class="material-symbols-outlined">add</span>
            </button>
          {:else}
            {#if app.cueDevice !== null}<span class="saved-action-space"
              ></span>{/if}
            {#if app.isAdmin}<span class="saved-action-space"></span>{/if}
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
          onclick={() => openSaved(saved.id)}
          onkeydown={(e) => onRowKeyDown(saved, e)}
          oncontextmenu={(e) => openMenu({ kind: "saved", saved }, e)}
          role="button"
          aria-haspopup="menu"
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
          {#if app.autoSource?.id === saved.id}
            <span
              class="material-symbols-outlined saved-row-source"
              title="Auto Mode draws its music from this saved playlist"
              aria-label="Auto-playlist source">auto_awesome</span
            >
          {/if}
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
{#if menu}
  <ContextMenu
    x={menu.x}
    y={menu.y}
    items={menuItems}
    label={menuLabel}
    onclose={closeMenu}
  />
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

  .saved-row-source {
    font-size: 16px;
    color: var(--primary);
  }

  .btn-filler.active {
    background: var(--primary);
    color: var(--on-primary);
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
