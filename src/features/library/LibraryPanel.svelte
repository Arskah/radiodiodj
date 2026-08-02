<script lang="ts">
  import { app, formatTime, type Track } from "../../shared/state.svelte";
  import type { ContentType, SortColumn } from "../../shared/types";

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

  function playNow(track: Track, e: MouseEvent): void {
    e.stopPropagation();
    app.playNow(track);
  }

  function add(track: Track, e: MouseEvent): void {
    e.stopPropagation();
    app.addToPlaylist(track);
  }

  function cues(track: Track, e: MouseEvent): void {
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
          oncontextmenu={(e) => {
            e.preventDefault();
            app.editingTrack = track;
          }}
          onmouseenter={(e) => onEnter(track, e)}
          onmouseleave={() => app.clearHover()}
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
          <button
            class="btn-add"
            title="Add to playlist"
            aria-label="Add to playlist"
            onclick={(e) => add(track, e)}
          >
            <span class="material-symbols-outlined">add</span>
          </button>
          {#if app.cueDevice !== null}
            <button
              class="btn-cue"
              title="Preview on cue deck"
              aria-label="Cue track"
              onclick={(e) => cues(track, e)}
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
            class="btn-play-track"
            title="Add and play"
            aria-label="Add and play"
            onclick={(e) => playNow(track, e)}
          >
            <span class="material-symbols-outlined">play_arrow</span>
          </button>
        </div>
      {/each}
    {/if}
  </div>
</section>
