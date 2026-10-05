<script lang="ts">
  import { app, formatTime } from "../../shared/state.svelte";
  import type { ContentType, Track } from "../../shared/types";
  import { airDuration } from "../../shared/cuePoints";
  import { findQuery } from "../../shared/savedPlaylists";

  const SEARCH_DELAY_MS = 150;

  const types: { type: ContentType; label: string }[] = [
    { type: "music", label: "Music" },
    { type: "commercial", label: "Commercials" },
    { type: "jingle", label: "Jingles" },
  ];

  let input: HTMLInputElement | undefined = $state();
  let query = $state("");
  let type = $state<ContentType>("music");
  let results = $state<Track[] | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let request = 0;

  const entry = $derived(app.findingFor);
  const sought = $derived(
    entry
      ? {
          title: entry.track?.title ?? entry.title,
          artist: entry.track?.artist ?? entry.artist,
          duration: entry.track ? airDuration(entry.track) : entry.duration,
        }
      : null,
  );

  $effect(() => {
    if (!entry) return;
    query = findQuery(entry);
    type = types.find((t) => t.type === entry.contentType)?.type ?? "music";
    results = null;
    error = null;
    busy = false;
    input?.focus();
    input?.select();
  });

  // Answers come back in any order, so only the newest one asked for is shown.
  $effect(() => {
    if (!entry) return;
    const asked = { query, type, request: ++request };
    const timer = setTimeout(() => {
      app
        .findTracks(asked.query, asked.type)
        .then((tracks) => {
          if (asked.request === request) results = tracks;
        })
        .catch((err: unknown) => {
          if (asked.request === request) error = messageOf(err);
        });
    }, SEARCH_DELAY_MS);
    return () => clearTimeout(timer);
  });

  const messageOf = (err: unknown): string =>
    err instanceof Error ? err.message : String(err);

  function close(): void {
    request++;
    app.findingFor = null;
  }

  async function bind(track: Track): Promise<void> {
    if (!entry || busy) return;
    busy = true;
    error = null;
    try {
      await app.bindSavedEntry(entry.id, track.id);
      close();
    } catch (err) {
      error = messageOf(err);
    } finally {
      busy = false;
    }
  }

  function onKeyDown(e: KeyboardEvent): void {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    }
  }
</script>

{#if entry && sought}
  <div
    class="dialog-scrim find-overlay"
    role="presentation"
    onmousedown={(e) => {
      if (e.target === e.currentTarget) close();
    }}
  >
    <div
      id="find-dialog"
      class="dialog-card find-content"
      role="dialog"
      aria-modal="true"
      aria-label="Find in library"
      tabindex="-1"
      onkeydown={onKeyDown}
    >
      <div class="dialog-header">
        <h2>Find in Library</h2>
        <button
          id="btn-find-close"
          class="btn-close"
          onclick={close}
          title="Close (Escape)"
        >
          <span class="material-symbols-outlined">close</span>
        </button>
      </div>
      <div class="find-body">
        <p class="find-desc">
          Pick the track this entry stands for
          <span class="find-sought"
            >{sought.artist} – {sought.title}
            <span class="find-sought-time">{formatTime(sought.duration)}</span
            ></span
          >
        </p>
        <div class="find-search">
          <span class="material-symbols-outlined" aria-hidden="true"
            >search</span
          >
          <input
            id="find-search"
            type="text"
            autocomplete="off"
            aria-label="Search the library"
            placeholder="Search tracks, artists, albums…"
            bind:this={input}
            bind:value={query}
          />
        </div>
      </div>
      <div class="find-tabs" role="tablist" aria-label="Type">
        {#each types as t (t.type)}
          <button
            type="button"
            class="find-tab"
            class:active={type === t.type}
            role="tab"
            aria-selected={type === t.type}
            onclick={() => (type = t.type)}>{t.label}</button
          >
        {/each}
      </div>
      <div class="saved-headers">
        <span class="track-header track-title">Title</span>
        <span class="track-header track-artist">Artist</span>
        <span class="track-header track-album">Album</span>
        <span class="track-header track-duration">Time</span>
      </div>
      <div id="find-results" class="find-results">
        {#if results === null}
          <div class="empty">
            <span class="empty-title">Searching…</span>
          </div>
        {:else if results.length === 0}
          <div class="empty">
            <span class="empty-icon"
              ><span class="material-symbols-outlined">search_off</span></span
            >
            <span class="empty-title">No Track Matches</span>
            <span class="empty-body">Try fewer words, or another type.</span>
          </div>
        {:else}
          {#each results as track (track.id)}
            <button
              type="button"
              class="track-row find-row"
              disabled={busy}
              data-track-id={track.id}
              onclick={() => bind(track)}
            >
              <span class="track-title">{track.title}</span>
              <span class="track-artist">{track.artist}</span>
              <span class="track-album">{track.album}</span>
              <span class="track-duration"
                >{formatTime(airDuration(track))}</span
              >
            </button>
          {/each}
        {/if}
      </div>
      {#if error}
        <div id="find-error" class="find-error" role="alert">{error}</div>
      {/if}
      <div class="find-footer">
        <button type="button" class="btn" onclick={close}>Cancel</button>
      </div>
    </div>
  </div>
{/if}

<style>
  /* Chrome is .dialog-scrim / .dialog-card in styles.css, and the list is the
     library's: .saved-headers over .track-row, with its column classes. */
  .find-overlay {
    --dialog-width: 680px;
  }

  .find-content {
    display: flex;
    flex-direction: column;
  }

  .find-body {
    padding: 16px 20px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .find-desc {
    margin: 0;
    font-size: 12px;
    color: var(--on-surface-variant);
  }

  .find-sought {
    display: block;
    margin-top: 2px;
    font-size: 14px;
    color: var(--on-surface);
  }

  .find-sought-time {
    margin-left: var(--sp-sm);
    color: var(--on-surface-variant);
    font-family: var(--font-mono);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }

  /* The library's search box. */
  .find-search {
    position: relative;
    display: flex;
    align-items: center;
  }

  .find-search .material-symbols-outlined {
    position: absolute;
    left: var(--sp-md);
    font-size: 18px;
    color: var(--outline);
    pointer-events: none;
  }

  .find-search input {
    flex: 1;
    min-width: 0;
    height: 30px;
    padding: 6px 14px 6px 40px;
    background: var(--surface-container-lowest);
    border: 1px solid
      color-mix(in srgb, var(--outline-variant) 30%, transparent);
    border-radius: var(--r-pill);
    color: var(--on-surface);
    font-size: 12px;
    font-family: inherit;
    outline: none;
  }

  .find-search input:focus {
    border-color: var(--primary);
    box-shadow: 0 0 0 1px var(--primary);
  }

  .find-search input::placeholder {
    color: var(--outline);
  }

  /* The library's tab row. */
  .find-tabs {
    display: flex;
    padding: 0 var(--sp-md);
    background: var(--surface-container-low);
    border-top: 1px solid
      color-mix(in srgb, var(--outline-variant) 30%, transparent);
    border-bottom: 1px solid
      color-mix(in srgb, var(--outline-variant) 30%, transparent);
  }

  .find-tab {
    padding: 8px 12px;
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    color: var(--on-surface-variant);
    cursor: pointer;
    font-size: 11px;
    font-weight: 700;
    letter-spacing: 0.06em;
    text-transform: uppercase;
  }

  .find-tab:hover {
    color: var(--on-surface);
  }

  .find-tab.active {
    color: var(--primary);
    border-bottom-color: var(--primary);
  }

  .saved-headers .track-header {
    cursor: default;
  }

  .find-results {
    display: flex;
    flex-direction: column;
    height: 320px;
    overflow-y: auto;
    padding: var(--sp-xs) 0;
    background: var(--surface-container-low);
  }

  .find-results .empty {
    margin: auto;
  }

  /* A row is a button here, where the library's is a div. */
  .find-row {
    flex-shrink: 0;
    width: 100%;
    border: 0;
    background: none;
    font-family: inherit;
    text-align: left;
    cursor: pointer;
  }

  .find-row:focus-visible {
    outline: none;
    background: color-mix(in srgb, var(--primary) 14%, transparent);
  }

  .find-row:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .find-error {
    margin: 12px 20px 0;
    color: var(--error);
    font-size: 13px;
    padding: 8px 10px;
    background: color-mix(in srgb, var(--error) 10%, transparent);
    border-radius: var(--r-lg);
  }

  .find-footer {
    display: flex;
    justify-content: flex-end;
    padding: 16px 20px;
    border-top: 1px solid var(--outline-variant);
  }

  .btn {
    background: transparent;
    border: 1px solid var(--outline-variant);
    color: var(--on-surface-variant);
    padding: 8px 16px;
    border-radius: var(--r-lg);
    cursor: pointer;
    font-size: 13px;
  }

  .btn:hover {
    background: color-mix(in srgb, var(--on-surface) 8%, transparent);
    color: var(--on-surface);
  }
</style>
