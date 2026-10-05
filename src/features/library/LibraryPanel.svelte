<script lang="ts">
  import { tick } from "svelte";
  import { app, formatTime, type Track } from "../../shared/state.svelte";
  import type { LibraryTab, SortColumn } from "../../shared/types";
  import { airDuration, isTrimmed } from "../../shared/cuePoints";
  import ContextMenu from "../ui/ContextMenu.svelte";
  import type { MenuItem } from "../ui/contextMenu";
  import { allSelected, hiddenCount } from "../../shared/selection";
  import SavedPlaylists from "./SavedPlaylists.svelte";

  const tabs: { type: LibraryTab; label: string }[] = [
    { type: "music", label: "Music" },
    { type: "commercial", label: "Commercials" },
    { type: "jingle", label: "Jingles" },
    { type: "playlists", label: "Playlists" },
  ];

  const onPlaylists = $derived(app.activeTab === "playlists");

  const sortableCols: { column: SortColumn; label: string; cls: string }[] = [
    // Album order rather than a bare number sort, so searching an album and
    // clicking # reads the record the way it was cut.
    { column: "track_no", label: "#", cls: "track-no" },
    { column: "title", label: "Title", cls: "track-title" },
    { column: "artist", label: "Artist", cls: "track-artist" },
    { column: "album", label: "Album", cls: "track-album" },
    { column: "play_count", label: "Plays", cls: "track-plays" },
  ];

  function sortIcon(column: SortColumn): string {
    if (app.sortBy !== column) return "unfold_more";
    return app.sortDir === "asc" ? "arrow_upward" : "arrow_downward";
  }

  function ariaSort(column: SortColumn): "ascending" | "descending" | "none" {
    if (app.sortBy !== column) return "none";
    return app.sortDir === "asc" ? "ascending" : "descending";
  }

  let searchTimeout: number | undefined;

  function onSearchInput(): void {
    clearTimeout(searchTimeout);
    searchTimeout = window.setTimeout(() => app.search(), 250);
  }

  /**
   * Enter leaves the search box for the first result, so the list can be
   * worked from the keyboard. The pending search is run first: the rows on
   * screen may still answer the query as it stood a keystroke ago.
   */
  async function focusResults(): Promise<void> {
    if (onPlaylists) return;
    clearTimeout(searchTimeout);
    await app.search();
    await tick();
    document.querySelector<HTMLElement>("#track-list .track-row")?.focus();
  }

  function onSearchKeyDown(e: KeyboardEvent): void {
    if (e.key !== "Enter") return;
    e.preventDefault();
    void focusResults();
  }

  function add(track: Track, e: MouseEvent): void {
    e.stopPropagation();
    app.addToPlaylist(track);
  }

  function cue(track: Track, e: MouseEvent): void {
    e.stopPropagation();
    app.cueLoad(track);
  }

  function startEdit(track: Track, e: MouseEvent): void {
    e.stopPropagation();
    app.editingMetadata = track;
  }

  function onEnter(track: Track, e: MouseEvent): void {
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    app.setHover(track, rect);
  }

  function onDragStart(track: Track, e: DragEvent): void {
    // The tooltip is anchored to the row and would hang over the drag.
    app.clearHover();
    app.startLibraryDrag(track);
    const count = app.draggedTrackIds?.length ?? 1;
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = "copy";
      e.dataTransfer.setData(
        "text/plain",
        count > 1 ? `${count} tracks` : `${track.artist} – ${track.title}`,
      );
    }
  }

  // ----- Selection (#576) -----

  const listedIds = $derived(app.listedIds);
  const selectedCount = $derived(app.selectedIds.length);
  const hidden = $derived(hiddenCount(app.selectedIds, listedIds));
  const allListed = $derived(allSelected(app.selectedIds, listedIds));
  const selectAllLabel = $derived(
    allListed ? "Deselect shown" : `Select all (${listedIds.length})`,
  );
  /** Each picked track's place in the order it will be queued in, from 1. */
  const ordinals = $derived(
    new Map(app.selectedIds.map((id, i) => [id, i + 1])),
  );

  function pick(track: Track, range: boolean): void {
    if (range) app.selectRangeTo(track.id);
    else app.toggleSelected(track.id);
  }

  // Both clicks of a double-click toggle the row first, so it is dropped here
  // whichever way they left it.
  function onRowDblClick(track: Track, e: MouseEvent): void {
    e.preventDefault();
    app.addToPlaylist(track);
    app.deselect(track.id);
  }

  /** A field that takes the keys itself, where Ctrl/Cmd+A selects its text. */
  function typing(target: EventTarget | null): boolean {
    return (
      target instanceof HTMLElement &&
      (target.isContentEditable ||
        target instanceof HTMLInputElement ||
        target instanceof HTMLTextAreaElement ||
        target instanceof HTMLSelectElement)
    );
  }

  // On the document, not the list: select-all means the library wherever
  // focus happens to be, and nothing has focus after a click on empty space.
  function onDocumentKeyDown(e: KeyboardEvent): void {
    if (typing(e.target) || menuTrack) return;
    if (app.settingsOpen || document.querySelector('[aria-modal="true"]')) {
      return;
    }
    if (e.key === "Escape" && selectedCount > 0) {
      app.clearSelection();
    } else if (e.key === "a" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      app.toggleSelectAll();
    }
  }

  $effect(() => {
    document.addEventListener("keydown", onDocumentKeyDown);
    return () => document.removeEventListener("keydown", onDocumentKeyDown);
  });

  // ----- Row context menu (#314) -----

  let menuTrack = $state<Track | null>(null);
  let menuX = $state(0);
  let menuY = $state(0);
  // The row the menu was opened from, so focus goes back where it came from.
  let menuRow: HTMLElement | null = null;

  /** `at` is the cursor point; without one the menu hangs off the row itself. */
  function showMenu(
    track: Track,
    row: HTMLElement,
    at: { x: number; y: number } | null,
  ): void {
    // The hover tooltip is anchored to the row and would sit under the menu.
    app.clearHover();
    const rect = row.getBoundingClientRect();
    menuX = at ? at.x : rect.left;
    menuY = at ? at.y : rect.bottom;
    menuRow = row;
    menuTrack = track;
  }

  function openMenu(track: Track, e: MouseEvent): void {
    e.preventDefault();
    // Windows and Linux dispatch a `contextmenu` event for the menu key with no
    // cursor position; anchor those to the row, not the viewport corner.
    const keyboard = e.clientX === 0 && e.clientY === 0;
    showMenu(
      track,
      e.currentTarget as HTMLElement,
      keyboard ? null : { x: e.clientX, y: e.clientY },
    );
  }

  // Windows and Linux deliver the menu key and Shift+F10 as `contextmenu`, but
  // macOS keyboards have neither — handle them here and bind Ctrl+Enter too, or
  // the menu is pointer-only there.
  function onRowKeyDown(track: Track, e: KeyboardEvent): void {
    // A button inside the row has Space of its own.
    if (e.key === " " && e.target === e.currentTarget) {
      e.preventDefault();
      pick(track, e.shiftKey);
      return;
    }
    const wants =
      e.key === "ContextMenu" ||
      (e.key === "F10" && e.shiftKey) ||
      (e.key === "Enter" && e.ctrlKey);
    if (!wants) return;
    e.preventDefault();
    showMenu(track, e.currentTarget as HTMLElement, null);
  }

  function closeMenu(restoreFocus: boolean): void {
    menuTrack = null;
    if (restoreFocus) menuRow?.focus();
    menuRow = null;
  }

  // Play-now sits last, behind a divider: #354 took it off the row because a
  // stray click must never reach air, so it is never the item under the cursor
  // when the menu opens.
  let menuItems = $derived.by<MenuItem[]>(() => {
    const track = menuTrack;
    if (!track) return [];
    if (selectedCount > 1 && ordinals.has(track.id)) {
      return [
        {
          label: `Add ${selectedCount} to playlist`,
          icon: "add",
          onselect: () => app.addSelectionToPlaylist(),
        },
        {
          label: `Add ${selectedCount} as next`,
          icon: "playlist_play",
          onselect: () => app.addSelectionToPlaylist(true),
        },
        {
          label: app.isAdmin
            ? `Add ${selectedCount} to saved playlist…`
            : `New saved playlist from ${selectedCount}…`,
          icon: "playlist_add",
          onselect: () => app.saveSelection(),
          separated: true,
        },
      ];
    }
    const items: MenuItem[] = [
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
    ];
    if (app.cueDevice !== null) {
      items.push({
        label: "Preview on cue deck",
        icon: "headphones",
        onselect: () => app.cueLoad(track),
      });
    }
    items.push({
      label: app.isAdmin ? "Add to saved playlist…" : "New saved playlist…",
      icon: "playlist_add",
      onselect: () =>
        (app.savedDialog = { kind: "addTo", trackIds: [track.id] }),
    });
    if (app.isAdmin) {
      items.push({
        label: "Edit metadata…",
        icon: "edit",
        onselect: () => (app.editingMetadata = track),
      });
    }
    items.push({
      label: "Cue points…",
      icon: "line_start_diamond",
      onselect: () => (app.editingCuePoints = track),
    });
    items.push({
      label: "Show in folder",
      icon: "folder_open",
      onselect: () => app.revealTrack(track),
      separated: true,
    });
    items.push({
      label: "Play now (on air)",
      icon: "play_arrow",
      onselect: () => app.playNow(track),
      separated: true,
      danger: true,
    });
    return items;
  });
</script>

<section id="library-panel" class="panel">
  <div class="panel-header">
    <span class="panel-title">
      <span class="material-symbols-outlined" aria-hidden="true"
        >library_music</span
      >
      Library
    </span>
    <div id="search-wrap">
      <span class="material-symbols-outlined" aria-hidden="true">search</span>
      <input
        type="text"
        id="search-input"
        placeholder={!onPlaylists
          ? "Search tracks, artists, albums…"
          : app.openSaved
            ? "Search this playlist…"
            : "Search saved playlists…"}
        autocomplete="off"
        aria-label={!onPlaylists
          ? "Search tracks"
          : app.openSaved
            ? "Search this saved playlist"
            : "Search saved playlists"}
        bind:value={app.searchQuery}
        oninput={onSearchInput}
        onkeydown={onSearchKeyDown}
      />
    </div>
  </div>
  <div id="library-tabs" role="tablist" aria-label="Library tabs">
    {#each tabs as { type, label } (type)}
      <button
        class="lib-tab"
        class:active={app.activeTab === type}
        data-type={type}
        role="tab"
        aria-selected={app.activeTab === type}
        onclick={() => app.setTab(type)}
      >
        {label}
      </button>
    {/each}
  </div>
  {#if !onPlaylists}
    <div id="track-headers">
      {#each sortableCols as col (col.column)}
        <button
          class="track-header {col.cls}"
          class:active={app.sortBy === col.column}
          role="columnheader"
          aria-sort={ariaSort(col.column)}
          onclick={() => app.toggleSort(col.column)}
        >
          {col.label}
          <span class="material-symbols-outlined" aria-hidden="true"
            >{sortIcon(col.column)}</span
          >
        </button>
      {/each}
      <span class="track-header track-duration">Time</span>
      <span class="track-header-spacer">
        <button
          id="btn-select-all"
          title={selectAllLabel}
          aria-label={selectAllLabel}
          disabled={app.tracks.length === 0}
          onclick={() => app.toggleSelectAll()}
        >
          <span class="material-symbols-outlined" aria-hidden="true"
            >{allListed ? "deselect" : "select_all"}</span
          >
        </button>
      </span>
    </div>
  {/if}
  <div id="track-list-wrap">
    {#if onPlaylists}
      <SavedPlaylists />
    {:else}
      <div id="track-list" class:selecting={selectedCount > 0}>
        {#if app.tracks.length === 0}
          <div class="empty">
            <span class="empty-icon"
              ><span class="material-symbols-outlined">library_music</span
              ></span
            >
            <span class="empty-title">Your Library is Empty</span>
            <span class="empty-body"
              >Add music, jingles, and commercials from Settings → Library, then
              scan to build your station.</span
            >
          </div>
        {:else}
          {#each app.tracks as track (track.id)}
            {@const ordinal = ordinals.get(track.id)}
            <div
              class="track-row"
              class:selected={ordinal !== undefined}
              draggable="true"
              ondragstart={(e) => onDragStart(track, e)}
              ondragend={() => (app.draggedTrackIds = null)}
              onclick={(e) => pick(track, e.shiftKey)}
              ondblclick={(e) => onRowDblClick(track, e)}
              onmouseenter={(e) => onEnter(track, e)}
              onmouseleave={() => app.clearHover()}
              oncontextmenu={(e) => openMenu(track, e)}
              onkeydown={(e) => onRowKeyDown(track, e)}
              role="button"
              aria-label={`Track: ${track.title} by ${track.artist}`}
              aria-haspopup="menu"
              aria-expanded={menuTrack?.id === track.id}
              aria-pressed={ordinal !== undefined}
              data-track-id={track.id}
              tabindex="0"
            >
              <span class="track-no">
                <span class="track-no-value">{track.track_no ?? ""}</span>
                <span class="track-check" aria-hidden="true"
                  >{ordinal ?? ""}</span
                >
              </span>
              <span class="track-title">{track.title}</span>
              <span class="track-artist">{track.artist}</span>
              <span class="track-album">{track.album}</span>
              <span class="track-plays">{track.play_count || 0}</span>
              <span class="track-duration" class:trimmed={isTrimmed(track)}
                >{formatTime(airDuration(track))}</span
              >
              {#if app.cueDevice !== null}
                <button
                  class="btn-cue"
                  title="Preview on cue deck"
                  aria-label="Cue track"
                  onclick={(e) => cue(track, e)}
                >
                  <span class="material-symbols-outlined">headphones</span>
                </button>
              {/if}
              {#if app.isAdmin}
                <button
                  class="btn-edit"
                  title="Edit metadata"
                  aria-label="Edit track metadata"
                  onclick={(e) => startEdit(track, e)}
                >
                  <span class="material-symbols-outlined">edit</span>
                </button>
              {/if}
              <button
                class="btn-add"
                title="Add to playlist"
                aria-label="Add to playlist"
                onclick={(e) => add(track, e)}
              >
                <span class="material-symbols-outlined">add</span>
              </button>
            </div>
          {/each}
        {/if}
      </div>
    {/if}
    {#if selectedCount > 0}
      <div id="selection-bar" role="toolbar" aria-label="Selection">
        <span id="selection-count" aria-live="polite">
          {selectedCount} selected{hidden > 0 ? ` · ${hidden} not shown` : ""}
        </span>
        <button
          class="btn-filler"
          id="btn-add-selection"
          onclick={() => app.addSelectionToPlaylist()}
          >Add {selectedCount} to playlist</button
        >
        <button
          class="btn-filler"
          id="btn-add-selection-next"
          onclick={() => app.addSelectionToPlaylist(true)}
          >Add {selectedCount} as next</button
        >
        <button
          class="btn-filler"
          id="btn-save-selection"
          title={app.isAdmin
            ? "Add the selection to a saved playlist, or make a new one of it"
            : "Make a new saved playlist of the selection"}
          onclick={() => app.saveSelection()}>Save…</button
        >
        <button
          class="btn-selection"
          id="btn-clear-selection"
          onclick={() => app.clearSelection()}>Clear</button
        >
      </div>
    {/if}
  </div>
  {#if menuTrack}
    <ContextMenu
      x={menuX}
      y={menuY}
      items={menuItems}
      label={selectedCount > 1 && ordinals.has(menuTrack.id)
        ? `Actions for ${selectedCount} selected tracks`
        : `Actions for ${menuTrack.title}`}
      onclose={closeMenu}
    />
  {/if}
</section>
