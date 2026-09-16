<script lang="ts">
  import { app } from "../../shared/state.svelte";
  import ErrorBanner from "../ui/ErrorBanner.svelte";

  async function openSettings(): Promise<void> {
    await app.loadLibraryPaths();
    app.settingsOpen = true;
  }
</script>

<!-- Row 1: thin draggable titlebar (macOS traffic lights sit in the left inset). -->
<header id="toolbar">
  <div id="toolbar-spacer"></div>
</header>

<!-- Row 2: production nav (brand, live badge, alert slot, stats, controls). -->
<nav id="app-nav">
  <div class="nav-left">
    <span class="brand">RadiodioDJ</span>
    <span class="live-badge">
      <span class="led"></span>
      Live On Air
    </span>
  </div>

  <div class="nav-center">
    {#if app.outputUnavailable}
      <ErrorBanner
        message="Audio output unavailable — retrying…"
        type="error"
      />
    {:else if app.cueOutputUnavailable}
      <ErrorBanner message="Cue output unavailable — retrying…" type="error" />
    {:else if app.reconnecting}
      <ErrorBanner message="Reconnecting…" type="warning" />
    {/if}
  </div>

  <div class="nav-right">
    {#if app.stats && app.stats.totalTracks > 0}
      <div class="nav-stats">
        <div class="stat">
          <span class="stat-label">Tracks</span>
          <span class="stat-value">{app.stats.totalTracks}</span>
        </div>
        <div class="stat">
          <span class="stat-label">Artists</span>
          <span class="stat-value">{app.stats.totalArtists}</span>
        </div>
        <div
          class="stat"
          title="Library size, in file length before cue points"
        >
          <span class="stat-label">Playtime</span>
          <span class="stat-value">{app.stats.totalHours}h</span>
        </div>
      </div>
    {/if}
    <button
      id="btn-generate"
      class:active={app.autoPlaylistActive}
      title="Generate random playlist"
      aria-pressed={app.autoPlaylistActive}
      onclick={() => app.toggleAutoPlaylist()}
    >
      <span class="material-symbols-outlined">auto_awesome</span>
      Auto Mode
    </button>
    <button
      id="btn-settings"
      class="nav-icon-btn"
      title="Settings — library paths, audio devices, scan"
      aria-label="Settings"
      onclick={openSettings}
    >
      <span class="material-symbols-outlined">settings</span>
    </button>
  </div>
</nav>
