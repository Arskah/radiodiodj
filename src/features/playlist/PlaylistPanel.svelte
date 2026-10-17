<script lang="ts">
  import {
    app,
    formatSpan,
    formatTime,
    type Track,
  } from "../../shared/state.svelte";
  import {
    isStopMarker,
    isTrackItem,
    type PlaylistItem,
    type QueueDestination,
  } from "../../shared/types";
  import { airDuration, airedTrack, isTrimmed } from "../../shared/cuePoints";
  import MissingBadge from "../track/MissingBadge.svelte";
  import { queueDropTarget, type QueueDropRow } from "./playlistDrop";
  import { sizeLabel } from "../../shared/savedPlaylists";
  import { blockTrackCount } from "../../shared/queueBlocks";

  let localDrag = $state<{ kind: "block" | "occurrence"; id: string } | null>(
    null,
  );
  let dropTarget = $state<QueueDestination | null>(null);

  function onEnter(track: Track, e: MouseEvent): void {
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    app.setHover(track, rect);
  }

  function onDragStart(
    e: DragEvent,
    kind: "block" | "occurrence",
    id: string,
  ): void {
    localDrag = { kind, id };
    // A library row removed mid-drag never fires the `dragend` that clears it.
    app.draggedTrackIds = null;
    app.draggedSavedPlaylistId = null;
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", id);
    }
  }

  function onDragEnd(): void {
    localDrag = null;
    dropTarget = null;
  }

  /** A row of this list or of the library is in the air. */
  const dragging = $derived(
    localDrag !== null ||
      app.draggedTrackIds !== null ||
      app.draggedSavedPlaylistId !== null,
  );

  // A drag that ends anywhere else never reaches a handler here.
  $effect(() => {
    if (!dragging) dropTarget = null;
  });

  // The same hazard the other way round: a queued row that aired mid-drag.
  $effect(() => {
    if (app.draggedTrackIds || app.draggedSavedPlaylistId !== null)
      localDrag = null;
  });

  /**
   * Resolved on the list, never on a row: the padding around the rows and the
   * space between them belong to the list, and a drop there is still a drop
   * at a position.
   */
  function destinationUnder(e: DragEvent): QueueDestination {
    const list = e.currentTarget as HTMLElement;
    const rows: QueueDropRow[] = Array.from(
      list.querySelectorAll<HTMLElement>(".playlist-row[data-queue-kind]"),
      (row) => {
        const rect = row.getBoundingClientRect();
        return {
          id: row.dataset.queueId ?? "",
          kind: row.dataset.queueKind as QueueDropRow["kind"],
          blockId: row.dataset.blockId ?? null,
          top: rect.top,
          height: rect.height,
          left: rect.left,
        };
      },
    );
    const source =
      localDrag?.kind === "block" || app.draggedSavedPlaylistId !== null
        ? "block"
        : "leaf";
    const target = queueDropTarget(
      e.clientY,
      rows,
      app.queue,
      source,
      e.clientX,
    );
    if (
      app.currentBlockId !== null &&
      target.scope === "main" &&
      target.beforeId === app.currentBlockId
    ) {
      const activeIndex = app.queue.findIndex(
        (node) => node.id === app.currentBlockId,
      );
      return {
        scope: "main",
        beforeId: app.queue[activeIndex + 1]?.id ?? null,
      };
    }
    return target;
  }

  function onDragOver(e: DragEvent): void {
    if (!dragging) return;
    e.preventDefault();
    if (e.dataTransfer) {
      e.dataTransfer.dropEffect =
        app.draggedTrackIds || app.draggedSavedPlaylistId !== null
          ? "copy"
          : "move";
    }
    dropTarget = destinationUnder(e);
  }

  function onDragLeave(e: DragEvent): void {
    const list = e.currentTarget as HTMLElement;
    if (!(e.relatedTarget instanceof Node) || !list.contains(e.relatedTarget)) {
      dropTarget = null;
    }
  }

  function onDrop(e: DragEvent): void {
    e.preventDefault();
    const destination = destinationUnder(e);
    const moved = localDrag;
    dropTarget = null;
    localDrag = null;
    if (moved?.kind === "block") {
      if (destination.scope === "main") {
        app.moveQueueBlock(moved.id, destination.beforeId);
      }
      return;
    }
    if (moved?.kind === "occurrence") {
      app.moveQueueOccurrence(moved.id, destination);
      return;
    }
    app.dropDraggedInQueue(destination);
  }

  const stopped = $derived(app.playlist.some(isStopMarker));
  const queuedTracks = $derived(app.playlist.filter(isTrackItem).length);

  const positions = $derived.by(() => {
    const byId = new Map<string, number>();
    let index = 0;
    for (const node of app.queue) {
      if (node.kind === "standalone") byId.set(node.id, ++index);
      else for (const child of node.entries) byId.set(child.id, ++index);
    }
    return byId;
  });

  const mainGap = (id: string | null): boolean =>
    dropTarget?.scope === "main" && dropTarget.beforeId === id;
  const childGap = (blockId: string, id: string | null): boolean =>
    dropTarget?.scope === "block" &&
    dropTarget.blockId === blockId &&
    dropTarget.beforeId === id;
  const lastInScope = (id: string, blockId: string | null): boolean => {
    if (blockId === null) return app.queue.at(-1)?.id === id;
    const block = app.queue.find((node) => node.id === blockId);
    return block?.kind === "block" && block.entries.at(-1)?.id === id;
  };
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
        <button
          id="btn-save-playlist"
          class="btn-filler"
          title="Keep the remaining upcoming entries as a saved list"
          disabled={app.playlist.length === 0}
          onclick={() => (app.savedDialog = { kind: "saveAs" })}>Save as</button
        >
        <button
          id="btn-clear-playlist"
          title="Clear playlist"
          onclick={() => app.clearPlaylist()}>Clear</button
        >
      {/if}
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
      {#if app.playlist.length > 0}
        <span
          class="pl-tab-total"
          title={stopped
            ? "Air time until the first stop marker"
            : "Air time of the queue"}>· {formatSpan(app.upcomingAirTime)}</span
        >
      {/if}
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

  {#if app.autoSource || app.revertedFrom}
    <div id="auto-source" role="status">
      <span class="material-symbols-outlined" aria-hidden="true"
        >auto_awesome</span
      >
      {#if app.autoSource}
        <span class="auto-source-text">
          Auto Mode draws from <strong>{app.autoSource.name}</strong> · {sizeLabel(
            app.autoSource.tracks,
            0,
          )}{#if app.sourceSwitchQueued && queuedTracks > 0}
            <span class="auto-source-queued">
              · {queuedTracks} queued air first</span
            >
          {/if}
        </span>
        {#if app.sourceSwitchQueued && queuedTracks > 0}
          <button
            id="btn-source-clear"
            class="btn-filler"
            title="Clear the playlist, so the source's tracks air next"
            onclick={() => app.clearPlaylist()}>Clear</button
          >
        {/if}
        <button
          id="btn-source-off"
          class="btn-edit"
          title="Back to the music library"
          aria-label="Draw from the music library again"
          onclick={() => void app.setAutoSource(null)}
        >
          <span class="material-symbols-outlined">close</span>
        </button>
      {:else}
        <span class="auto-source-text">
          <strong>{app.revertedFrom}</strong> has no playable music. Auto Mode is
          back on the music library.
        </span>
        <button
          id="btn-source-dismiss"
          class="btn-edit"
          title="Dismiss"
          aria-label="Dismiss"
          onclick={() => void app.setAutoSource(null)}
        >
          <span class="material-symbols-outlined">close</span>
        </button>
      {/if}
    </div>
  {/if}

  {#if app.playlistTab === "playlist"}
    {#snippet leafRow(id: string, item: PlaylistItem, blockId: string | null)}
      {@const position = positions.get(id) ?? 0}
      {#if isStopMarker(item)}
        <div
          class="playlist-row stop-row queue-leaf"
          class:queue-child={blockId !== null}
          class:dragging={localDrag?.id === id}
          class:drop-before={dragging &&
            (blockId ? childGap(blockId, id) : mainGap(id))}
          class:drop-after={dragging &&
            blockId !== null &&
            lastInScope(id, blockId) &&
            childGap(blockId, null)}
          draggable="true"
          data-queue-kind={blockId ? "child" : "standalone"}
          data-queue-id={id}
          data-block-id={blockId ?? undefined}
          ondragstart={(e) => onDragStart(e, "occurrence", id)}
          ondragend={onDragEnd}
          role="listitem"
        >
          <span class="pl-drag"
            ><span class="material-symbols-outlined">drag_indicator</span></span
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
              app.removeQueueOccurrence(id);
            }}><span class="material-symbols-outlined">close</span></button
          >
        </div>
      {:else}
        {@const aired = airedTrack(item.track, item.cue_override)}
        <div
          class="playlist-row queue-leaf"
          class:queue-child={blockId !== null}
          class:dragging={localDrag?.id === id}
          class:drop-before={dragging &&
            (blockId ? childGap(blockId, id) : mainGap(id))}
          class:drop-after={dragging &&
            blockId !== null &&
            lastInScope(id, blockId) &&
            childGap(blockId, null)}
          draggable="true"
          data-queue-kind={blockId ? "child" : "standalone"}
          data-queue-id={id}
          data-block-id={blockId ?? undefined}
          ondragstart={(e) => onDragStart(e, "occurrence", id)}
          ondragend={onDragEnd}
          role="listitem"
        >
          <div
            class="pl-hover-area"
            role="presentation"
            onmouseenter={(e) => onEnter(aired, e)}
            onmouseleave={() => app.clearHover()}
          >
            <span class="pl-drag"
              ><span class="material-symbols-outlined">drag_indicator</span
              ></span
            >
            <span class="pl-num">{position}</span>
            <div class="pl-body">
              <span class="pl-title">{item.track.title}</span>
              <span class="pl-artist">{item.track.artist}</span>
            </div>
            <div class="pl-right">
              <MissingBadge trackId={item.track.id} />
              {#if position === 1}<span class="pl-status next-up">Next Up</span
                >{/if}
              {#if item.cue_override}
                <button
                  class="pl-override"
                  title="Custom cue points for this airing — click to play the track's own instead"
                  aria-label="Clear this airing's cue points"
                  onclick={(e) => {
                    e.stopPropagation();
                    app.setQueueOccurrenceCuePoints(id, null);
                  }}
                  ><span class="material-symbols-outlined"
                    >line_start_diamond</span
                  ></button
                >
              {/if}
              <span class="pl-duration" class:trimmed={isTrimmed(aired)}
                >{formatTime(airDuration(aired))}</span
              >
            </div>
          </div>
          <button
            class="btn-play-track"
            title="Play now"
            aria-label="Play now"
            onclick={(e) => {
              e.stopPropagation();
              app.playQueueOccurrence(id);
            }}><span class="material-symbols-outlined">play_arrow</span></button
          >
          <button
            class="btn-remove"
            title="Remove"
            aria-label="Remove from playlist"
            onclick={(e) => {
              e.stopPropagation();
              app.removeQueueOccurrence(id);
            }}><span class="material-symbols-outlined">close</span></button
          >
        </div>
      {/if}
    {/snippet}
    <div
      id="playlist"
      class:drop-into={dragging && app.queue.length === 0}
      ondragover={onDragOver}
      ondragleave={onDragLeave}
      ondrop={onDrop}
      role="list"
    >
      {#if app.queue.length === 0}
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
        {#each app.queue as node (node.id)}
          {#if node.kind === "standalone"}
            {@render leafRow(node.id, node.item, null)}
          {:else}
            {@const collapsed = app.collapsedBlockIds.includes(node.id)}
            {@const active = app.currentBlockId === node.id}
            <div
              class="playlist-row queue-block"
              class:active
              class:dragging={localDrag?.id === node.id}
              class:drop-before={dragging && mainGap(node.id)}
              class:drop-into={dragging &&
                (collapsed || node.entries.length === 0) &&
                dropTarget?.scope === "block" &&
                dropTarget.blockId === node.id}
              draggable={!active}
              data-queue-kind="block"
              data-queue-id={node.id}
              ondragstart={(e) => onDragStart(e, "block", node.id)}
              ondragend={onDragEnd}
              role="listitem"
            >
              <button
                class="queue-disclosure"
                aria-label={`${collapsed ? "Expand" : "Collapse"} ${node.title}`}
                aria-expanded={!collapsed}
                onclick={() => app.toggleBlockCollapsed(node.id)}
                ><span class="material-symbols-outlined"
                  >{collapsed ? "chevron_right" : "expand_more"}</span
                ></button
              >
              <span
                class="material-symbols-outlined queue-block-icon"
                aria-hidden="true">queue_music</span
              >
              <span class="queue-block-name">{node.title}</span>
              {#if active}<span class="pl-status next-up">Playing</span>{/if}
              <span class="queue-block-count"
                >{blockTrackCount(node)}
                {blockTrackCount(node) === 1 ? "track" : "tracks"}</span
              >
              <button
                class="btn-remove"
                aria-label={`Remove ${node.title} block`}
                title="Remove remaining block entries"
                onclick={() => app.removeQueueBlock(node.id)}
                ><span class="material-symbols-outlined">close</span></button
              >
            </div>
            {#if !collapsed}
              {#each node.entries as child (child.id)}
                {@render leafRow(child.id, child.item, node.id)}
              {/each}
            {/if}
          {/if}
        {/each}
        <div
          class="queue-main-end"
          class:drop-before={dragging && mainGap(null)}
          aria-hidden="true"
        ></div>
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
                <MissingBadge trackId={track.id} />
                <span class="pl-duration" class:trimmed={isTrimmed(track)}
                  >{formatTime(airDuration(track))}</span
                >
              </div>
            </div>
          </div>
        {/each}
      {/if}
    </div>
  {/if}
</section>

<style>
  .queue-block {
    gap: var(--sp-sm);
    background: var(--surface-container);
  }

  .queue-block.active {
    position: sticky;
    top: 0;
    z-index: 2;
    background: var(--surface-container-high);
  }

  .queue-child {
    margin-left: var(--sp-xl);
    width: calc(100% - var(--sp-xl));
    border-left: 2px solid var(--outline-variant);
  }

  .queue-disclosure {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    border: 0;
    background: transparent;
    color: var(--on-surface-variant);
    cursor: pointer;
  }

  .queue-disclosure .material-symbols-outlined {
    font-size: 18px;
  }

  .queue-block-icon {
    font-size: 18px;
    color: var(--primary);
  }

  .queue-block-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13px;
    font-weight: 600;
  }

  .queue-block-count {
    color: var(--on-surface-variant);
    font-family: var(--font-mono);
    font-size: 11px;
    white-space: nowrap;
  }

  .queue-block.drop-into {
    outline: 2px solid var(--primary);
    outline-offset: -2px;
  }

  .queue-main-end {
    height: 8px;
  }

  .queue-main-end.drop-before {
    border-top: 2px solid var(--primary);
  }
</style>
