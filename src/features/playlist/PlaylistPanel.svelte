<script lang="ts">
  import { app, formatTime, type Track } from "../../shared/state.svelte";
  import { isStopMarker, type PlaylistItem } from "../../shared/types";

  let dragFromIndex = $state(-1);
  let dropTarget = $state(-1);

  function onEnter(track: Track, e: MouseEvent): void {
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    app.setHover(track, rect);
  }

  function onDragStart(e: DragEvent, i: number): void {
    dragFromIndex = i;
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", String(i));
    }
  }

  function onDragEnd(): void {
    dragFromIndex = -1;
    dropTarget = -1;
  }

  function rowDropTarget(e: DragEvent, i: number): number {
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    return e.clientY < rect.top + rect.height / 2 ? i : i + 1;
  }

  function onRowDragOver(e: DragEvent, i: number): void {
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
    dropTarget = rowDropTarget(e, i);
  }

  function onRowDrop(e: DragEvent, i: number): void {
    e.preventDefault();
    const target = rowDropTarget(e, i);
    const from = dragFromIndex;
    dropTarget = -1;
    dragFromIndex = -1;
    if (from === -1) return;
    const insertAt = target > from ? target - 1 : target;
    if (insertAt === from) return;
    app.movePlaylistItem(from, insertAt);
  }

  function onListDragOver(e: DragEvent): void {
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
    if (dropTarget === -1) dropTarget = app.playlist.length;
  }

  function onListDrop(e: DragEvent): void {
    e.preventDefault();
    const from = dragFromIndex;
    const target = dropTarget === -1 ? app.playlist.length : dropTarget;
    dropTarget = -1;
    dragFromIndex = -1;
    if (from === -1) return;
    const insertAt = target > from ? target - 1 : target;
    if (insertAt === from) return;
    app.movePlaylistItem(from, insertAt);
  }

  function onClear(): void {
    if (app.playlistTab === "playlist") app.clearPlaylist();
    else app.clearHistory();
  }

  function rowKey(item: PlaylistItem): string {
    return isStopMarker(item) ? "stop" : String(item.track.id);
  }
</script>

<section id="playlist-panel" class="panel">
  <div class="panel-header">
    <span class="panel-title">
      <span class="material-symbols-outlined" aria-hidden="true"
        >queue_music</span
      >
      Active Playlist
    </span>
    <div id="playlist-actions">
      {#if app.playlistTab === "playlist"}
        <button
          class="btn-filler"
          title="Add a jingle to the playlist"
          disabled={(app.stats?.tracksByType.jingle ?? 0) === 0}
          onclick={() => app.addFiller("jingle")}>+ Jingle</button
        >
        <button
          class="btn-filler"
          title="Add a commercial to the playlist"
          disabled={(app.stats?.tracksByType.commercial ?? 0) === 0}
          onclick={() => app.addFiller("commercial")}>+ Comm</button
        >
        <button
          class="btn-filler btn-filler-stop"
          title="Stop automatic play when reached"
          onclick={() => app.addStopMarker()}>+ Stop</button
        >
      {/if}
      <button
        id="btn-clear-playlist"
        title={app.playlistTab === "playlist"
          ? "Clear playlist"
          : "Clear history"}
        onclick={onClear}>Clear</button
      >
    </div>
  </div>
  <div id="playlist-tabs" role="tablist">
    <button
      class="pl-tab"
      class:active={app.playlistTab === "playlist"}
      role="tab"
      aria-selected={app.playlistTab === "playlist"}
      onclick={() => (app.playlistTab = "playlist")}
    >
      Upcoming <span class="pl-tab-count">({app.playlist.length})</span>
    </button>
    <button
      class="pl-tab"
      class:active={app.playlistTab === "history"}
      role="tab"
      aria-selected={app.playlistTab === "history"}
      onclick={() => (app.playlistTab = "history")}
    >
      History <span class="pl-tab-count">({app.history.length})</span>
    </button>
  </div>

  {#if app.playlistTab === "playlist"}
    <div
      id="playlist"
      ondragover={onListDragOver}
      ondrop={onListDrop}
      role="list"
    >
      {#if app.playlist.length === 0}
        <div class="empty">
          <span class="empty-icon"
            ><span class="material-symbols-outlined">queue_music</span></span
          >
          <span class="empty-title">No Tracks Queued</span>
          <span class="empty-body"
            >Add tracks from the library, or enable Auto Mode to fill the queue
            automatically.</span
          >
        </div>
      {:else}
        {#each app.playlist as item, i (i + "-" + rowKey(item))}
          {#if isStopMarker(item)}
            <div
              class="playlist-row stop-row"
              class:dragging={i === dragFromIndex}
              class:drop-before={dragFromIndex !== -1 && dropTarget === i}
              class:drop-after={dragFromIndex !== -1 &&
                dropTarget === i + 1 &&
                i === app.playlist.length - 1}
              draggable="true"
              data-index={i}
              ondblclick={() => app.playIndex(i)}
              ondragstart={(e) => onDragStart(e, i)}
              ondragend={onDragEnd}
              ondragover={(e) => onRowDragOver(e, i)}
              ondrop={(e) => onRowDrop(e, i)}
              role="listitem"
            >
              <span class="pl-drag"
                ><span class="material-symbols-outlined">drag_indicator</span
                ></span
              >
              <span class="pl-num"
                ><span class="material-symbols-outlined">block</span></span
              >
              <div class="pl-body">
                <span class="pl-stop-label">Stop</span>
                <span class="pl-stop-sub">Auto play halts here</span>
              </div>
              <button
                class="btn-remove"
                title="Remove"
                aria-label="Remove stop marker"
                onclick={(e) => {
                  e.stopPropagation();
                  app.removeFromPlaylist(i);
                }}
              >
                <span class="material-symbols-outlined">close</span>
              </button>
            </div>
          {:else}
            <div
              class="playlist-row"
              class:dragging={i === dragFromIndex}
              class:drop-before={dragFromIndex !== -1 && dropTarget === i}
              class:drop-after={dragFromIndex !== -1 &&
                dropTarget === i + 1 &&
                i === app.playlist.length - 1}
              draggable="true"
              data-index={i}
              ondblclick={() => app.playIndex(i)}
              ondragstart={(e) => onDragStart(e, i)}
              ondragend={onDragEnd}
              ondragover={(e) => onRowDragOver(e, i)}
              ondrop={(e) => onRowDrop(e, i)}
              role="listitem"
            >
              <div
                class="pl-hover-area"
                role="presentation"
                onmouseenter={(e) => onEnter(item.track, e)}
                onmouseleave={() => app.clearHover()}
              >
                <span class="pl-drag"
                  ><span class="material-symbols-outlined">drag_indicator</span
                  ></span
                >
                <span class="pl-num">{i + 1}</span>
                <div class="pl-body">
                  <span class="pl-title">{item.track.title}</span>
                  <span class="pl-artist">{item.track.artist}</span>
                </div>
                <div class="pl-right">
                  {#if i === 0}
                    <span class="pl-status next-up">Next Up</span>
                  {/if}
                  <span class="pl-duration"
                    >{formatTime(item.track.duration)}</span
                  >
                </div>
              </div>
              <button
                class="btn-play-track"
                title="Play now"
                aria-label="Play now"
                onclick={(e) => {
                  e.stopPropagation();
                  app.playIndex(i);
                }}
              >
                <span class="material-symbols-outlined">play_arrow</span>
              </button>
              <button
                class="btn-remove"
                title="Remove"
                aria-label="Remove from playlist"
                onclick={(e) => {
                  e.stopPropagation();
                  app.removeFromPlaylist(i);
                }}
              >
                <span class="material-symbols-outlined">close</span>
              </button>
            </div>
          {/if}
        {/each}
      {/if}
    </div>
  {:else}
    <div id="history">
      {#if app.historyDisplay.length === 0}
        <div class="empty">
          <span class="empty-icon"
            ><span class="material-symbols-outlined">history</span></span
          >
          <span class="empty-title">No History Yet</span>
          <span class="empty-body">Tracks you play will appear here.</span>
        </div>
      {:else}
        {#each app.historyDisplay as track, i (i + "-" + track.id)}
          <div
            class="playlist-row history-row"
            data-index={i}
            ondblclick={() => app.requeueFromHistory(i)}
            role="listitem"
          >
            <div
              class="pl-hover-area"
              role="presentation"
              onmouseenter={(e) => onEnter(track, e)}
              onmouseleave={() => app.clearHover()}
            >
              <span class="pl-num">{i + 1}</span>
              <div class="pl-body">
                <span class="pl-title">{track.title}</span>
                <span class="pl-artist">{track.artist}</span>
              </div>
              <div class="pl-right">
                <span class="pl-duration">{formatTime(track.duration)}</span>
              </div>
            </div>
            <button
              class="btn-remove"
              title="Remove from history"
              aria-label="Remove from history"
              onclick={(e) => {
                e.stopPropagation();
                app.removeFromHistory(i);
              }}
            >
              <span class="material-symbols-outlined">close</span>
            </button>
          </div>
        {/each}
      {/if}
    </div>
  {/if}
</section>
