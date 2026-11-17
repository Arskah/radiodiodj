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
  } from "../../shared/types";
  import { airDuration, airedTrack, isTrimmed } from "../../shared/cuePoints";
  import MissingBadge from "../track/MissingBadge.svelte";
  import StopDurationField from "./StopDurationField.svelte";
  import { gapAt, moveTarget } from "./playlistDrop";
  import {
    dropAt,
    dropInto,
    groupColour,
    groupGap,
    upcomingRows,
    type HeaderRow,
    type UpcomingRow,
  } from "./playlistGroups";
  import { sizeLabel } from "../../shared/savedPlaylists";
  import { formatEta, msToNextSecond } from "../../shared/clock";
  import { DeckAnchor, etaDeck, upcomingEtas } from "../../shared/eta";

  let dragFromIndex = $state(-1);
  /** The group being dragged whole by its header. */
  let dragGroup = $state<string | null>(null);
  /** The gap between drawn rows a drop would land in. */
  let dropGap = $state(-1);
  /** The collapsed header a drop would append to, instead of a gap. */
  let dropHeader = $state<string | null>(null);

  /** Collapsed groups, by id. The window's alone: a restart opens them all. */
  let collapsed = $state<ReadonlySet<string>>(new Set());

  const rows = $derived(upcomingRows(app.playlist, app.onAirGroup, collapsed));

  let now = $state(Date.now());
  // On the second, as the toolbar clock ticks, so the two read alike.
  $effect(() => {
    let timer: ReturnType<typeof setTimeout>;
    const tick = (): void => {
      now = Date.now();
      timer = setTimeout(tick, msToNextSecond(now));
    };
    timer = setTimeout(tick, msToNextSecond(now));
    return () => clearTimeout(timer);
  });

  const anchor = new DeckAnchor();

  const etas = $derived.by(() => {
    // The tick's `now` can be most of a second old when a position arrives.
    const nowMs = Math.max(now, Date.now());
    const deck = app.currentTrack
      ? etaDeck(
          airedTrack(app.currentTrack, app.currentCueOverride),
          anchor.read(app.currentTime, app.isPlaying, nowMs),
          app.duration,
        )
      : null;
    return upcomingEtas({
      items: app.playlist,
      deck,
      hold: app.holding,
      autoAdvance: app.autoAdvance,
      nowMs,
    });
  });

  function toggleCollapsed(id: string): void {
    const next = new Set(collapsed);
    if (!next.delete(id)) next.add(id);
    collapsed = next;
  }

  const colourClass = (colour: number | null): string =>
    colour === null ? "" : `in-group group-colour-${colour}`;

  function rowKeyOf(row: UpcomingRow): string {
    if (row.kind === "header") return `h-${row.group.id}-${row.start}`;
    return `${row.index}-${rowKey(row.item)}`;
  }

  function onEnter(track: Track, e: MouseEvent): void {
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    app.setHover(track, rect);
  }

  function onDragStart(e: DragEvent, i: number): void {
    dragFromIndex = i;
    dragGroup = null;
    // A library row removed mid-drag never fires the `dragend` that clears it.
    app.endDrag();
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", String(i));
    }
  }

  function onGroupDragStart(e: DragEvent, id: string): void {
    dragGroup = id;
    dragFromIndex = -1;
    app.endDrag();
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", id);
    }
  }

  function onDragEnd(): void {
    dragFromIndex = -1;
    dragGroup = null;
    dropGap = -1;
    dropHeader = null;
  }

  /** A whole saved playlist is in the air, to queue as a group of its own. */
  const draggingSaved = $derived(app.draggedSavedId !== null);

  /** A row of this list, of the library or a saved playlist is in the air. */
  const dragging = $derived(
    dragFromIndex !== -1 ||
      dragGroup !== null ||
      app.draggedTrackIds !== null ||
      draggingSaved,
  );

  // A drag that ends anywhere else never reaches a handler here.
  $effect(() => {
    if (!dragging) {
      dropGap = -1;
      dropHeader = null;
    }
  });

  // The same hazard the other way round: a queued row that aired mid-drag.
  $effect(() => {
    if (app.draggedTrackIds || draggingSaved) {
      dragFromIndex = -1;
      dragGroup = null;
    }
  });

  /**
   * Resolved on the list, never on a row: the padding around the rows and the
   * space between them belong to the list, and a drop there is still a drop
   * at a position.
   */
  function gapUnder(e: DragEvent): number {
    const list = e.currentTarget as HTMLElement;
    const drawn = Array.from(list.querySelectorAll(".playlist-row"), (row) =>
      row.getBoundingClientRect(),
    );
    return gapAt(e.clientY, drawn);
  }

  /** A collapsed header under the pointer takes rows, though not a group. */
  function headerUnder(e: DragEvent): HeaderRow | null {
    if (dragGroup !== null || draggingSaved) return null;
    if (!(e.target instanceof Element)) return null;
    const id = e.target
      .closest(".group-header.collapsed")
      ?.getAttribute("data-group");
    return (
      rows.find(
        (row): row is HeaderRow => row.kind === "header" && row.group.id === id,
      ) ?? null
    );
  }

  function onDragOver(e: DragEvent): void {
    if (!dragging) return;
    e.preventDefault();
    if (e.dataTransfer) {
      e.dataTransfer.dropEffect =
        app.draggedTrackIds || draggingSaved ? "copy" : "move";
    }
    const header = headerUnder(e);
    dropHeader = header?.group.id ?? null;
    dropGap = header ? -1 : gapUnder(e);
  }

  function onDragLeave(e: DragEvent): void {
    const list = e.currentTarget as HTMLElement;
    if (!(e.relatedTarget instanceof Node) || !list.contains(e.relatedTarget)) {
      dropGap = -1;
      dropHeader = null;
    }
  }

  function onDrop(e: DragEvent): void {
    e.preventDefault();
    const header = headerUnder(e);
    const gap = gapUnder(e);
    const from = dragFromIndex;
    const group = dragGroup;
    const fromOutside = app.draggedTrackIds !== null;
    const saved = draggingSaved;
    onDragEnd();
    if (group !== null) {
      app.moveGroup(group, groupGap(rows, gap));
      return;
    }
    if (saved) {
      app.dropDraggedInPlaylist(groupGap(rows, gap));
      return;
    }
    const target = header ? dropInto(header) : dropAt(rows, gap);
    if (fromOutside) {
      app.dropDraggedInPlaylist(target.index, target.group);
      return;
    }
    if (from === -1) return;
    app.movePlaylistItem(
      from,
      moveTarget(from, target.index) ?? from,
      target.group,
    );
  }

  const stopped = $derived(app.playlist.some(isStopMarker));
  const queuedTracks = $derived(app.playlist.filter(isTrackItem).length);

  function rowKey(item: PlaylistItem): string {
    return isStopMarker(item) ? "stop" : String(item.track.id);
  }
</script>

{#snippet eta(i: number)}
  <span class="pl-eta" title="Expected start">{formatEta(etas[i] ?? null)}</span
  >
{/snippet}

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
          title="Keep the upcoming tracks as a saved playlist"
          disabled={!app.playlist.some(isTrackItem)}
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
    <div
      id="playlist"
      class:drop-into={dragging && dropGap === 0 && rows.length === 0}
      ondragover={onDragOver}
      ondragleave={onDragLeave}
      ondrop={onDrop}
      role="list"
    >
      {#if rows.length === 0}
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
        {#each rows as row, r (rowKeyOf(row))}
          {#if row.kind === "header"}
            {@const colour = groupColour(row.group.id)}
            <div
              class="playlist-row group-header group-colour-{colour}"
              class:collapsed={row.collapsed}
              class:on-air={row.onAir}
              class:pinned={row.pinned}
              class:dragging={dragGroup === row.group.id}
              class:drop-before={dragging && dropGap === r}
              class:drop-after={dragging &&
                dropGap === r + 1 &&
                r === rows.length - 1}
              class:drop-into={dragging && dropHeader === row.group.id}
              draggable={!row.onAir}
              data-group={row.group.id}
              ondragstart={(e) => onGroupDragStart(e, row.group.id)}
              ondragend={onDragEnd}
              role="listitem"
            >
              <button
                class="group-toggle"
                aria-expanded={!row.collapsed}
                aria-label={`${row.collapsed ? "Expand" : "Collapse"} ${row.group.name}`}
                title={row.collapsed ? "Expand" : "Collapse"}
                onclick={() => toggleCollapsed(row.group.id)}
              >
                <span class="material-symbols-outlined"
                  >{row.collapsed ? "chevron_right" : "expand_more"}</span
                >
              </button>
              <span class="material-symbols-outlined group-icon"
                >queue_music</span
              >
              <div class="pl-body">
                <span class="group-name">{row.group.name}</span>
                <span class="group-size"
                  >{sizeLabel(row.tracks + row.stops, 0, row.stops)}</span
                >
              </div>
              {#if row.onAir}
                <span class="pl-status group-on-air">On air</span>
              {/if}
              {#if row.end > row.start}
                {@render eta(row.start)}
              {/if}
              <button
                class="btn-edit"
                title="Save this group as a saved playlist"
                aria-label={`Save ${row.group.name} as a saved playlist`}
                disabled={row.tracks + row.stops === 0}
                onclick={() =>
                  (app.savedDialog = { kind: "saveAs", group: row.group })}
              >
                <span class="material-symbols-outlined">save</span>
              </button>
              <button
                class="btn-remove"
                title="Remove what is left of this group"
                aria-label={`Remove ${row.group.name}`}
                disabled={row.tracks + row.stops === 0}
                onclick={() => app.removeGroup(row.group.id)}
              >
                <span class="material-symbols-outlined">close</span>
              </button>
            </div>
          {:else}
            {@const i = row.index}
            {@const item = row.item}
            {@const colour = item.group ? groupColour(item.group.id) : null}
            {#if isStopMarker(item)}
              <div
                class="playlist-row stop-row {colourClass(colour)}"
                class:dragging={i === dragFromIndex}
                class:drop-before={dragging && dropGap === r}
                class:drop-after={dragging &&
                  dropGap === r + 1 &&
                  r === rows.length - 1}
                draggable="true"
                data-index={i}
                ondragstart={(e) => onDragStart(e, i)}
                ondragend={onDragEnd}
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
                <div class="pl-right">
                  {@render eta(i)}
                  <StopDurationField
                    value={item.planned_duration_seconds ?? null}
                    oncommit={(seconds) => app.setStopDuration(i, seconds)}
                  />
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
              {@const aired = airedTrack(item.track, item.cue_override)}
              <div
                class="playlist-row {colourClass(colour)}"
                class:dragging={i === dragFromIndex}
                class:drop-before={dragging && dropGap === r}
                class:drop-after={dragging &&
                  dropGap === r + 1 &&
                  r === rows.length - 1}
                draggable="true"
                data-index={i}
                ondragstart={(e) => onDragStart(e, i)}
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
                    ><span class="material-symbols-outlined"
                      >drag_indicator</span
                    ></span
                  >
                  <span class="pl-num">{i + 1}</span>
                  <div class="pl-body">
                    <span class="pl-title">{item.track.title}</span>
                    <span class="pl-artist">{item.track.artist}</span>
                  </div>
                  <div class="pl-right">
                    <MissingBadge trackId={item.track.id} />
                    {#if i === 0}
                      <span class="pl-status next-up">Next Up</span>
                    {/if}
                    {#if item.cue_override}
                      <!-- This airing plays under cue points of its own, not the
                         track's. Clicking hands it back to the radio edit. -->
                      <button
                        class="pl-override"
                        title="Custom cue points for this airing — click to play the track's own instead"
                        aria-label="Clear this airing's cue points"
                        onclick={(e) => {
                          e.stopPropagation();
                          app.setItemCuePoints(i, null);
                        }}
                      >
                        <span class="material-symbols-outlined"
                          >line_start_diamond</span
                        >
                      </button>
                    {/if}
                    {@render eta(i)}
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
