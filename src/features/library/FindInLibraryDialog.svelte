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
      <h2 class="find-title">Find in library</h2>
      <p class="find-desc">
        Pick the track this entry stands for:
        <span class="find-sought"
          >{sought.artist} – {sought.title} · {formatTime(
            sought.duration,
          )}</span
        >
      </p>
      <input
        id="find-search"
        type="text"
        autocomplete="off"
        aria-label="Search the library"
        placeholder="Search the library…"
        bind:this={input}
        bind:value={query}
      />
      <div class="find-types" role="tablist" aria-label="Type">
        {#each types as t (t.type)}
          <button
            type="button"
            class="find-type"
            class:active={type === t.type}
            role="tab"
            aria-selected={type === t.type}
            onclick={() => (type = t.type)}>{t.label}</button
          >
        {/each}
      </div>
      <div id="find-results" class="find-results">
        {#if results === null}
          <p class="find-empty">Searching…</p>
        {:else if results.length === 0}
          <p class="find-empty">No track matches. Try fewer words.</p>
        {:else}
          {#each results as track (track.id)}
            <button
              type="button"
              class="find-row"
              disabled={busy}
              data-track-id={track.id}
              onclick={() => bind(track)}
            >
              <span class="find-row-title">{track.title}</span>
              <span class="find-row-artist">{track.artist}</span>
              <span class="find-row-album">{track.album}</span>
              <span class="find-row-time">{formatTime(airDuration(track))}</span
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
  .find-overlay {
    --dialog-width: 640px;
  }

  .find-content {
    padding: 20px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .find-title {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
    color: var(--on-surface);
  }

  .find-desc {
    margin: 0;
    font-size: 13px;
    color: var(--on-surface-variant);
  }

  .find-sought {
    display: block;
    margin-top: 2px;
    color: var(--on-surface);
  }

  input {
    background: transparent;
    border: 1px solid var(--outline-variant);
    border-radius: var(--r-lg);
    padding: 8px 10px;
    color: var(--on-surface);
    font-size: 14px;
    outline: none;
  }

  input:focus {
    border-color: var(--primary);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--primary) 15%, transparent);
  }

  .find-types {
    display: flex;
    gap: 4px;
  }

  .find-type {
    background: transparent;
    border: 1px solid var(--outline-variant);
    border-radius: var(--r-lg);
    padding: 4px 10px;
    color: var(--on-surface-variant);
    font-size: 12px;
    cursor: pointer;
  }

  .find-type.active {
    background: var(--primary-container);
    border-color: var(--primary-container);
    color: var(--on-primary-container);
  }

  .find-results {
    display: flex;
    flex-direction: column;
    height: 320px;
    overflow-y: auto;
    border: 1px solid var(--outline-variant);
    border-radius: var(--r-lg);
  }

  .find-empty {
    margin: auto;
    font-size: 13px;
    color: var(--on-surface-variant);
  }

  .find-row {
    display: flex;
    align-items: center;
    gap: var(--sp-md);
    flex-shrink: 0;
    padding: 7px 10px;
    background: transparent;
    border: 0;
    color: var(--on-surface);
    font-size: 13px;
    text-align: left;
    cursor: pointer;
  }

  .find-row:hover:not(:disabled),
  .find-row:focus-visible {
    background: color-mix(in srgb, var(--on-surface) 8%, transparent);
    outline: none;
  }

  .find-row span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .find-row-title {
    flex: 3;
  }

  .find-row-artist,
  .find-row-album {
    flex: 2;
    color: var(--on-surface-variant);
  }

  .find-row-time {
    width: 48px;
    text-align: right;
    font-variant-numeric: tabular-nums;
    color: var(--on-surface-variant);
  }

  .find-error {
    color: var(--error);
    font-size: 13px;
    padding: 8px 10px;
    background: color-mix(in srgb, var(--error) 10%, transparent);
    border-radius: var(--r-lg);
  }

  .find-footer {
    display: flex;
    justify-content: flex-end;
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
