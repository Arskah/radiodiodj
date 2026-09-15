<script lang="ts">
  import { app, formatTime, type Track } from "../../shared/state.svelte";
  import type { ContentType, SortColumn } from "../../shared/types";
  import ContextMenu from "../ui/ContextMenu.svelte";
  import type { MenuItem } from "../ui/contextMenu";

  const tabs: { type: ContentType; label: string }[] = [
    { type: "music", label: "Music" },
    { type: "commercial", label: "Commercials" },
    { type: "jingle", label: "Jingles" },
  ];

  const sortableCols: { column: SortColumn; label: string; cls: string }[] = [
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

  function add(track: Track, e: MouseEvent): void {
    e.stopPropagation();
    app.addToPlaylist(track);
  }

  function cue(track: Track, e: MouseEvent): void {
    e.stopPropagation();
    app.cueLoadAndPlay(track);
  }

  function startEdit(track: Track, e: MouseEvent): void {
    e.stopPropagation();
    app.editingTrack = track;
  }

  function onEnter(track: Track, e: MouseEvent): void {
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    app.setHover(track, rect);
  }

  // ----- Row context menu (#314) -----

  let menuTrack = $state<Track | null>(null);
  let menuX = $state(0);
  let menuY = $state(0);
  // The row the menu was opened from, so focus goes back where it came from.
  let menuRow: HTMLElement | null = null;

  function openMenu(track: Track, e: MouseEvent): void {
    e.preventDefault();
    // The hover tooltip is anchored to the row and would sit under the menu.
    app.clearHover();
    const row = e.currentTarget as HTMLElement;
    // The keyboard menu key fires `contextmenu` with no cursor position; anchor
    // those to the row's bottom-left instead of the viewport corner.
    const keyboard = e.clientX === 0 && e.clientY === 0;
    const rect = row.getBoundingClientRect();
    menuX = keyboard ? rect.left : e.clientX;
    menuY = keyboard ? rect.bottom : e.clientY;
    menuRow = row;
    menuTrack = track;
  }

  function closeMenu(restoreFocus: boolean): void {
    menuTrack = null;
    if (restoreFocus) menuRow?.focus();
    menuRow = null;
  }

  // Mirrors the row's action buttons, plus the play-now that #354 took off the
  // row: reaching air deliberately is fine, reaching it with a stray click is
  // not. It sits last, behind a divider, so it is never the item under the
  // cursor when the menu opens.
  let menuItems = $derived.by<MenuItem[]>(() => {
    const track = menuTrack;
    if (!track) return [];
    const items: MenuItem[] = [
      {
        label: "Add to playlist",
        icon: "add",
        onselect: () => app.addToPlaylist(track),
      },
    ];
    if (app.cueDevice !== null) {
      items.push({
        label: "Preview on cue deck",
        icon: "headphones",
        onselect: () => app.cueLoadAndPlay(track),
      });
    }
    items.push({
      label: "Edit metadata…",
      icon: "edit",
      onselect: () => (app.editingTrack = track),
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
        placeholder="Search tracks, artists, albums…"
        autocomplete="off"
        aria-label="Search tracks"
        bind:value={app.searchQuery}
        oninput={onSearchInput}
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
    <span class="track-header-spacer"></span>
  </div>
  <div id="track-list">
    {#if app.tracks.length === 0}
      <div class="empty">
        <span class="empty-icon"
          ><span class="material-symbols-outlined">library_music</span></span
        >
        <span class="empty-title">Your Library is Empty</span>
        <span class="empty-body"
          >Add music, jingles, and commercials from Settings → Library Sync,
          then scan to build your station.</span
        >
      </div>
    {:else}
      {#each app.tracks as track (track.id)}
        <div
          class="track-row"
          ondblclick={(e) => {
            e.preventDefault();
            app.addToPlaylist(track);
          }}
          onmouseenter={(e) => onEnter(track, e)}
          onmouseleave={() => app.clearHover()}
          oncontextmenu={(e) => openMenu(track, e)}
          role="button"
          aria-label={`Track: ${track.title} by ${track.artist}`}
          data-track-id={track.id}
          tabindex="0"
        >
          <span class="track-title">{track.title}</span>
          <span class="track-artist">{track.artist}</span>
          <span class="track-album">{track.album}</span>
          <span class="track-plays">{track.play_count || 0}</span>
          <span class="track-duration">{formatTime(track.duration)}</span>
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
          <button
            class="btn-edit"
            title="Edit metadata"
            aria-label="Edit track metadata"
            onclick={(e) => startEdit(track, e)}
          >
            <span class="material-symbols-outlined">edit</span>
          </button>
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
  {#if menuTrack}
    <ContextMenu
      x={menuX}
      y={menuY}
      items={menuItems}
      label={`Actions for ${menuTrack.title}`}
      onclose={closeMenu}
    />
  {/if}
</section>
