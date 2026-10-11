<script lang="ts">
  import { app } from "../../shared/state.svelte";
  import ErrorBanner from "../ui/ErrorBanner.svelte";

  // The version rides along because a locked desk cannot open About, and
  // whoever reports a bug from there still needs to say what they are running.
  const settingsTitle = $derived.by(() => {
    const notes: string[] = [];
    if (app.healthAttention > 0) {
      notes.push(`the library needs attention (${app.healthAttention})`);
    }
    if (app.hubAttention) {
      const waiting = app.sharedLibrary.status.waiting;
      notes.push(
        waiting > 0
          ? `the shared library needs attention (${waiting} waiting to be sent)`
          : "the shared library needs attention",
      );
    }
    if (app.update.offer) {
      notes.push(`version ${app.update.offer.version} is available`);
    }
    if (!app.isAdmin) notes.push("unlock admin mode");
    const what =
      notes.length > 0
        ? notes.join("; ")
        : "library paths, audio devices, scan";
    const version = app.update.currentVersion;
    return `Settings — ${what}${version ? ` · v${version}` : ""}`;
  });

  function toggleLock(): void {
    if (app.isAdmin) app.lockAdmin();
    else app.unlockOpen = true;
  }

  function openSettings(): void {
    if (!app.isAdmin) return;
    app.settingsOpen = true;
  }
</script>

<!-- Row 1: thin draggable titlebar (macOS traffic lights sit in the left inset). -->
<header id="toolbar">
  <div id="toolbar-spacer"></div>
</header>

<!-- Row 2: production nav (brand, alert slot, stats, controls). -->
<nav id="app-nav">
  <div class="nav-left">
    <!-- A logo replaces the name rather than sitting beside it; the name it
         replaces survives as alt text and in the window title. -->
    {#if app.appearance?.logo}
      <img class="brand-logo" src={app.appearance.logo} alt={app.brandName} />
    {:else}
      <span class="brand">{app.brandName}</span>
    {/if}
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
    {:else if app.libraryReset}
      <ErrorBanner
        message="Library rebuilt for this version — rescanning…"
        type="warning"
      />
    {:else if app.libraryJoined}
      <ErrorBanner
        message="Joined the shared library — fetching it from the hub…"
        type="warning"
      />
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
    {#if app.admin.passwordSet}
      <button
        id="btn-admin-lock"
        class="nav-icon-btn"
        class:active={app.isAdmin}
        title={app.isAdmin
          ? "Admin mode unlocked — click to lock"
          : "Admin mode locked — click to unlock"}
        aria-label={app.isAdmin ? "Lock admin mode" : "Unlock admin mode"}
        aria-pressed={app.isAdmin}
        onclick={toggleLock}
      >
        <span class="material-symbols-outlined"
          >{app.isAdmin ? "lock_open" : "lock"}</span
        >
      </button>
    {/if}
    <button
      id="btn-settings"
      class="nav-icon-btn"
      disabled={!app.isAdmin}
      title={settingsTitle}
      aria-label={app.healthAttention > 0
        ? `Settings, ${app.healthAttention} library issues`
        : app.hubAttention
          ? "Settings, the shared library needs attention"
          : "Settings"}
      onclick={openSettings}
    >
      <span class="material-symbols-outlined">settings</span>
      {#if app.healthAttention > 0}
        <span class="attention-badge attention-badge--corner" aria-hidden="true"
          >{app.healthAttention}</span
        >
      {:else if app.hubAttention}
        <span class="attention-badge attention-badge--corner" aria-hidden="true"
          >!</span
        >
      {:else if app.updateWaiting}
        <span
          class="attention-badge attention-badge--corner attention-badge--update"
          aria-hidden="true">↑</span
        >
      {/if}
    </button>
  </div>
</nav>
