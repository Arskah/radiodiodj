<script lang="ts">
  import { app, formatTime } from "../../shared/state.svelte";
  import { checkHasChanges, formatAgo, plural } from "../../shared/health";
  import { airDuration, hasCuePoints, isTrimmed } from "../../shared/cuePoints";
  import { isTrackItem } from "../../shared/types";
  import type {
    DuplicateGroup,
    FindingKind,
    MissingTrack,
  } from "../../shared/types";

  /** Longest path list drawn per disk-change kind. */
  const PATH_LIMIT = 200;

  let selected = $state(new Set<number>());
  let confirming = $state<number[] | null>(null);
  let purging = $state(false);

  const report = $derived(app.health);
  const check = $derived(report.check);
  const scanning = $derived(app.scanStatus.status === "running");

  const deleted = $derived(report.missing.filter((t) => !t.outsideRoots));
  const unrooted = $derived(report.missing.filter((t) => t.outsideRoots));

  const queuedIds = $derived(
    new Set([
      ...app.playlist.filter(isTrackItem).map((i) => i.track.id),
      ...(app.currentTrack ? [app.currentTrack.id] : []),
    ]),
  );

  // A purge or a scan changes the list; forget choices for rows that left it.
  $effect(() => {
    const ids = new Set(report.missing.map((t) => t.id));
    const kept = [...selected].filter((id) => ids.has(id));
    if (kept.length !== selected.size) selected = new Set(kept);
  });

  function toggle(id: number): void {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    selected = next;
  }

  function toggleAll(tracks: MissingTrack[]): void {
    const all = tracks.every((t) => selected.has(t.id));
    const next = new Set(selected);
    for (const t of tracks) {
      if (all) next.delete(t.id);
      else next.add(t.id);
    }
    selected = next;
  }

  const confirmTracks = $derived(
    confirming === null
      ? []
      : report.missing.filter((t) => confirming!.includes(t.id)),
  );

  async function purge(): Promise<void> {
    if (confirming === null) return;
    purging = true;
    await app.purgeTracks(confirming);
    purging = false;
    confirming = null;
  }

  function dismissToggle(
    kind: FindingKind,
    dismissed: boolean,
    key = "",
  ): void {
    if (dismissed) app.undismissFinding(kind, key);
    else app.dismissFinding(kind, key);
  }

  function groupTitle(group: DuplicateGroup): string {
    const t = group.tracks[0]?.track;
    return t ? `${t.title} — ${t.artist}` : group.key;
  }

  async function scanNow(): Promise<void> {
    await app.scan();
  }
</script>

{#snippet dismissButton(kind: FindingKind, dismissed: boolean, key: string)}
  <button
    class="health-dismiss"
    title={dismissed
      ? "Show this in the badge again"
      : "Stop counting this in the badge until it changes"}
    onclick={(e) => {
      // Inside a <summary>, a click would also fold the group.
      e.preventDefault();
      dismissToggle(kind, dismissed, key);
    }}>{dismissed ? "Undo dismiss" : "Dismiss"}</button
  >
{/snippet}

{#snippet pathList(label: string, paths: string[])}
  {#if paths.length > 0}
    <details class="health-paths">
      <summary>{label} ({paths.length})</summary>
      <ul>
        {#each paths.slice(0, PATH_LIMIT) as p (p)}
          <li>{p}</li>
        {/each}
        {#if paths.length > PATH_LIMIT}
          <li class="health-more">…and {paths.length - PATH_LIMIT} more</li>
        {/if}
      </ul>
    </details>
  {/if}
{/snippet}

{#snippet missingRow(t: MissingTrack)}
  <label class="health-row" class:selected={selected.has(t.id)}>
    <input
      type="checkbox"
      checked={selected.has(t.id)}
      onchange={() => toggle(t.id)}
    />
    <span class="health-name">
      <span class="health-title">{t.title}</span>
      <span class="health-sub">{t.artist}</span>
    </span>
    <span class="health-path" title={t.path}>{t.path}</span>
    <span class="health-when" title={new Date(t.missingSince).toLocaleString()}
      >{formatAgo(t.missingSince)}</span
    >
    <span class="health-icons">
      {#if t.hasCuePoints}
        <span
          class="material-symbols-outlined"
          title="Has cue points a purge would delete">line_start_diamond</span
        >
      {/if}
      <span class="health-plays" title="Play count">▶{t.playCount}</span>
      {#if queuedIds.has(t.id)}
        <span class="material-symbols-outlined" title="In the playlist"
          >queue_music</span
        >
      {/if}
    </span>
  </label>
{/snippet}

{#snippet group(kind: "exact" | "possible", g: DuplicateGroup)}
  <details
    class="health-group"
    class:dismissed={g.dismissed}
    open={!g.dismissed}
  >
    <summary>
      <span class="health-group-title">{groupTitle(g)}</span>
      <span class="health-sub">{plural(g.tracks.length, "copy", "copies")}</span
      >
      {#if g.dismissed}<span class="health-tag">Dismissed</span>{/if}
      {@render dismissButton(kind, g.dismissed, g.key)}
    </summary>
    {#each g.tracks as m (m.track.id)}
      <div class="health-row">
        <span class="health-name">
          <span class="health-title">{m.track.title}</span>
          <span class="health-sub">{m.track.artist}</span>
        </span>
        <span class="health-path" title={m.path}>{m.path}</span>
        <span class="health-type">{m.contentType}</span>
        <span class="health-duration" class:trimmed={isTrimmed(m.track)}
          >{formatTime(airDuration(m.track))}</span
        >
        <span class="health-icons">
          {#if hasCuePoints(m.track.cue_points)}
            <span class="material-symbols-outlined" title="Has cue points"
              >line_start_diamond</span
            >
          {/if}
          <span class="health-plays" title="Play count"
            >▶{m.track.play_count}</span
          >
        </span>
        <span class="health-actions">
          <button
            title="Show in folder"
            aria-label="Show in folder"
            onclick={() => app.revealTrack(m.track)}
            ><span class="material-symbols-outlined">folder_open</span></button
          >
          <button
            title="Edit metadata…"
            aria-label="Edit metadata"
            onclick={() => (app.editingMetadata = m.track)}
            ><span class="material-symbols-outlined">edit</span></button
          >
          <button
            title="Cue points…"
            aria-label="Cue points"
            onclick={() => (app.editingCuePoints = m.track)}
            ><span class="material-symbols-outlined">line_start_diamond</span
            ></button
          >
        </span>
      </div>
    {/each}
  </details>
{/snippet}

<div class="settings-section" id="library-health">
  <h4>Library Health</h4>
  <p class="settings-section-desc">
    Tracks whose files are gone, copies of the same recording, and changes on
    disk that no scan has picked up yet. Nothing here changes an audio file:
    delete an unwanted copy in the file manager, then scan.
  </p>

  <section class="health-section" id="health-disk">
    <header>
      <h5 class="tuning-group-title">Disk changes</h5>
      {#if check && checkHasChanges(check)}
        {@render dismissButton("check", report.checkDismissed, "")}
      {/if}
    </header>
    {#each check?.unreachable ?? [] as root (root)}
      <p class="health-warning" role="alert">
        <span class="material-symbols-outlined" aria-hidden="true">warning</span
        >
        {root} is unreachable. Its tracks are kept as they are.
      </p>
    {/each}
    {#each check?.partial ?? [] as root (root)}
      <p class="health-note">
        Parts of {root} could not be read, so nothing under it is reported gone.
      </p>
    {/each}
    <p class="health-summary">
      {#if check === null}
        Not checked since the last scan.
      {:else if checkHasChanges(check)}
        {check.new.length} new · {check.changed.length} changed · {check.gone
          .length} gone{#if check.unrooted.length > 0}
          · {check.unrooted.length} outside library paths{/if}
        — checked {formatAgo(check.checkedAt)}
      {:else}
        No changes — checked {formatAgo(check.checkedAt)}
      {/if}
    </p>
    {#if check}
      {@render pathList("New", check.new)}
      {@render pathList("Changed", check.changed)}
      {@render pathList("Gone", check.gone)}
      {@render pathList("No longer under a library path", check.unrooted)}
    {/if}
    <div class="health-buttons">
      <button class="btn-scan-now" disabled={scanning} onclick={scanNow}>
        <span class="material-symbols-outlined">sync</span>
        {scanning ? "Scanning…" : "Scan now"}
      </button>
      <button
        class="btn-purge-cancel"
        disabled={scanning}
        onclick={() => app.checkLibraryNow()}>Check now</button
      >
    </div>
  </section>

  <section class="health-section" id="health-missing">
    <header>
      <h5 class="tuning-group-title">
        Missing tracks ({report.missing.length})
      </h5>
      {#if report.missing.length > 0}
        {@render dismissButton("missing", report.missingDismissed, "")}
      {/if}
    </header>
    {#if report.missing.length === 0}
      <p class="health-summary">No issues.</p>
    {:else}
      <p class="health-note">
        Hidden from the library. A track comes back with its cue points and play
        count if its file reappears, even under a new name or folder.
      </p>
      {#each deleted as t (t.id)}
        {@render missingRow(t)}
      {/each}
      {#if unrooted.length > 0}
        <details class="health-group">
          <summary>
            <input
              type="checkbox"
              aria-label="Select every track outside the library paths"
              checked={unrooted.every((t) => selected.has(t.id))}
              onclick={(e) => e.stopPropagation()}
              onchange={() => toggleAll(unrooted)}
            />
            <span class="health-group-title"
              >No longer under a library path ({unrooted.length})</span
            >
          </summary>
          {#each unrooted as t (t.id)}
            {@render missingRow(t)}
          {/each}
        </details>
      {/if}
      {#if confirming !== null}
        {@const cued = confirmTracks.filter((t) => t.hasCuePoints).length}
        {@const queued = confirmTracks.filter((t) =>
          queuedIds.has(t.id),
        ).length}
        <div class="missing-tracks-confirm" role="alert">
          <span>
            Delete {plural(confirmTracks.length, "track")} for good{#if cued > 0},
              with the cue points of {cued}{/if}{#if queued > 0}, and take {queued}
              off the playlist{/if}?
          </span>
          <button
            id="btn-purge-confirm"
            class="btn-purge-confirm"
            disabled={purging || scanning}
            onclick={purge}>Delete</button
          >
          <button
            class="btn-purge-cancel"
            disabled={purging}
            onclick={() => (confirming = null)}>Keep</button
          >
        </div>
      {:else}
        <div class="health-buttons">
          <button
            id="btn-purge-selected"
            class="btn-purge"
            disabled={scanning || selected.size === 0}
            title={scanning ? "Wait for the scan to finish" : undefined}
            onclick={() => (confirming = [...selected])}
            >Purge selected ({selected.size})…</button
          >
          <button
            id="btn-purge-missing"
            class="btn-purge"
            disabled={scanning}
            title={scanning ? "Wait for the scan to finish" : undefined}
            onclick={() => (confirming = report.missing.map((t) => t.id))}
            >Purge all…</button
          >
        </div>
      {/if}
    {/if}
  </section>

  <section class="health-section" id="health-duplicates">
    <header>
      <h5 class="tuning-group-title">Duplicates</h5>
    </header>
    {#if report.unhashed > 0}
      <p class="health-note">
        Still checking {plural(report.unhashed, "track")} for exact copies…
      </p>
    {/if}
    {#if report.exact.length === 0 && report.possible.length === 0}
      <p class="health-summary">No issues.</p>
    {/if}
    {#if report.exact.length > 0}
      <h6>Exact copies ({report.exact.length})</h6>
      <p class="health-note">Same audio at more than one path.</p>
      {#each report.exact as g (g.key)}
        {@render group("exact", g)}
      {/each}
    {/if}
    {#if report.possible.length > 0}
      <h6>Possible duplicates ({report.possible.length})</h6>
      <p class="health-note">
        Same artist and title, different audio. Usually the same song in another
        encoding or edit — check before deleting.
      </p>
      {#each report.possible as g (g.key)}
        {@render group("possible", g)}
      {/each}
    {/if}
  </section>
</div>

<style>
  .health-section {
    margin-top: var(--sp-lg);
  }

  .health-section header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-sm);
  }

  h6 {
    margin: var(--sp-md) 0 var(--sp-xs);
    font-size: 12px;
    color: var(--on-surface);
  }

  .health-summary,
  .health-note,
  .health-warning {
    margin: var(--sp-xs) 0;
    font-size: 12px;
    color: var(--on-surface-variant);
  }

  .health-warning {
    display: flex;
    align-items: center;
    gap: var(--sp-xs);
    color: var(--error);
  }

  .health-buttons {
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-sm);
    margin-top: var(--sp-sm);
  }

  .health-dismiss {
    padding: 2px var(--sp-sm);
    background: none;
    border: 1px solid var(--outline-variant);
    border-radius: var(--r-lg);
    color: var(--on-surface-variant);
    font-size: 11px;
    cursor: pointer;
  }

  .health-dismiss:hover {
    color: var(--on-surface);
  }

  .health-row {
    display: flex;
    align-items: center;
    gap: var(--sp-sm);
    padding: var(--sp-xs) var(--sp-sm);
    border-radius: var(--r-lg);
    font-size: 12px;
  }

  .health-row:hover,
  .health-row.selected {
    background: color-mix(in srgb, var(--surface-variant) 30%, transparent);
  }

  .health-name {
    display: flex;
    flex-direction: column;
    flex: 0 1 12rem;
    min-width: 0;
  }

  .health-title,
  .health-group-title {
    color: var(--on-surface);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .health-sub {
    color: var(--on-surface-variant);
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .health-path {
    flex: 1 1 0;
    min-width: 0;
    color: var(--on-surface-variant);
    font-family: var(--font-mono);
    font-size: 11px;
    direction: rtl;
    text-align: left;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .health-when,
  .health-type,
  .health-duration,
  .health-plays {
    color: var(--on-surface-variant);
    white-space: nowrap;
  }

  .health-duration.trimmed {
    color: var(--cue-in-color);
  }

  .health-icons,
  .health-actions {
    display: flex;
    align-items: center;
    gap: var(--sp-xs);
  }

  .health-icons .material-symbols-outlined,
  .health-actions .material-symbols-outlined {
    font-size: 16px;
  }

  .health-actions button {
    display: inline-flex;
    padding: 2px;
    background: none;
    border: none;
    border-radius: var(--r-lg);
    color: var(--on-surface-variant);
    cursor: pointer;
  }

  .health-actions button:hover {
    color: var(--primary);
  }

  .health-group {
    margin: var(--sp-xs) 0;
    padding: var(--sp-xs) var(--sp-sm);
    border: 1px solid var(--outline-variant);
    border-radius: var(--r-lg);
  }

  .health-group.dismissed {
    opacity: 0.6;
  }

  .health-group summary {
    display: flex;
    align-items: center;
    gap: var(--sp-sm);
    font-size: 12px;
    cursor: pointer;
  }

  .health-group summary .health-dismiss {
    margin-left: auto;
  }

  .health-tag {
    padding: 0 var(--sp-xs);
    border: 1px solid var(--outline-variant);
    border-radius: var(--r-pill);
    font-size: 10px;
    color: var(--on-surface-variant);
  }

  .health-paths {
    margin: var(--sp-xs) 0;
    font-size: 12px;
    color: var(--on-surface-variant);
  }

  .health-paths summary {
    cursor: pointer;
  }

  .health-paths ul {
    margin: var(--sp-xs) 0 0;
    padding-left: var(--sp-lg);
    max-height: 12rem;
    overflow-y: auto;
    font-family: var(--font-mono);
    font-size: 11px;
  }

  .health-more {
    list-style: none;
    font-style: italic;
  }
</style>
