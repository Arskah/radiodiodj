<script lang="ts">
  import { app } from "../../shared/state.svelte";
  import { formatAgo } from "../../shared/health";
  import { replacesLibrary, roleLabel } from "../../shared/sharedLibrary";
  import type { LibraryRole } from "../../shared/types";

  const roles: LibraryRole[] = ["standalone", "owner", "studio"];

  // A draft, saved by its own button: a role is not something to change with
  // a stray click, and nothing here applies before a restart anyway.
  let role = $state<LibraryRole>("standalone");
  let url = $state("");
  let machineName = $state("");
  let showUrl = $state(false);
  let confirming = $state(false);
  let saving = $state(false);
  let error = $state<string | null>(null);

  const saved = $derived(app.sharedLibrary);
  const running = $derived(saved.status.role);
  const dirty = $derived(
    role !== saved.role ||
      url.trim() !== (saved.url ?? "") ||
      machineName.trim() !== (saved.machineName ?? ""),
  );
  const needsUrl = $derived(role !== "standalone" && url.trim() === "");
  // What is saved is not what is running until the app has been restarted. A
  // role with no address runs as not shared.
  const savedTakesEffect = $derived(
    saved.url === null ? "standalone" : saved.role,
  );
  const restartPending = $derived(savedTakesEffect !== running);

  function adopt(): void {
    role = app.sharedLibrary.role;
    url = app.sharedLibrary.url ?? "";
    machineName = app.sharedLibrary.machineName ?? "";
    confirming = false;
    error = null;
  }

  $effect(() => {
    if (app.settingsOpen) void app.loadSharedLibrary().then(adopt);
  });

  async function save(): Promise<void> {
    if (replacesLibrary(role, running, saved.role) && !confirming) {
      confirming = true;
      return;
    }
    saving = true;
    error = null;
    try {
      await app.saveSharedLibrary(
        role,
        url.trim() || null,
        machineName.trim() || null,
      );
      adopt();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      saving = false;
    }
  }
</script>

<div class="settings-section settings-section--tuning">
  <h4>Shared library</h4>
  <p class="settings-section-desc">
    One library for several computers. The <em>library owner</em> scans the
    files and measures the audio; a <em>studio</em> plays from a copy of the owner's
    library and never scans. Both connect out to a database, the hub, that sits between
    them. Every computer keeps its own copy, so playback never waits on the network.
  </p>

  <div class="device-row">
    <label for="shared-role">This computer</label>
    <span class="select-wrap">
      <select
        id="shared-role"
        value={role}
        onchange={(e) => {
          role = e.currentTarget.value as LibraryRole;
          confirming = false;
        }}
      >
        {#each roles as r (r)}
          <option value={r}>{roleLabel(r)}</option>
        {/each}
      </select>
    </span>
    <div class="hint">
      {#if role === "owner"}
        Scans the library paths and publishes the library to the hub. One
        computer at a time.
      {:else if role === "studio"}
        Plays from a copy of the owner's library. Scanning, the analysis pass,
        purging and the library paths themselves are the owner's; where each
        path is on this computer is still set under Library.
      {:else}
        The library is this computer's own and no hub is contacted.
      {/if}
    </div>
  </div>

  {#if role !== "standalone"}
    <div class="np-field">
      <label class="np-field-label" for="shared-url">Hub address</label>
      <div class="np-input-wrap">
        <span class="material-symbols-outlined">database</span>
        <input
          id="shared-url"
          class="np-input mono"
          type={showUrl ? "text" : "password"}
          placeholder="postgresql://user:password@host:5432/database"
          autocapitalize="off"
          autocorrect="off"
          autocomplete="off"
          spellcheck="false"
          bind:value={url}
        />
        <button
          type="button"
          class="np-eye"
          title={showUrl ? "Hide address" : "Show address"}
          aria-label={showUrl ? "Hide address" : "Show address"}
          onclick={() => (showUrl = !showUrl)}
        >
          <span class="material-symbols-outlined"
            >{showUrl ? "visibility_off" : "visibility"}</span
          >
        </button>
      </div>
      <div class="np-file-hint">
        A PostgreSQL connection URL. Its password is stored in plain text in
        this computer's <code>config.json</code>.
      </div>
    </div>
    <div class="np-field">
      <label class="np-field-label" for="shared-name"
        >Name of this computer</label
      >
      <div class="np-input-wrap">
        <span class="material-symbols-outlined">computer</span>
        <input
          id="shared-name"
          class="np-input"
          type="text"
          placeholder="Studio 1"
          bind:value={machineName}
        />
      </div>
      <div class="np-file-hint">What the other computers see it as.</div>
    </div>
  {/if}

  {#if confirming}
    <div class="missing-tracks-confirm" role="alert">
      <span>
        Becoming a studio replaces this computer's library the next time
        RadiodioDJ starts. Its tracks, cue points, edits and saved playlists are
        kept in <code>radiodiodj.standalone.bak.db</code> and are not in the copy
        that replaces them. Do this off air.
      </span>
      <button
        id="btn-shared-confirm"
        class="btn-purge-confirm"
        disabled={saving}
        onclick={save}>Replace at next start</button
      >
      <button
        class="btn-purge-cancel"
        disabled={saving}
        onclick={() => (confirming = false)}>Cancel</button
      >
    </div>
  {:else}
    <div class="np-action-row">
      <button
        id="btn-shared-save"
        class="btn-scan-now"
        disabled={!dirty || needsUrl || saving}
        title={needsUrl ? "A shared library needs a hub address" : undefined}
        onclick={save}>{saving ? "Saving…" : "Save"}</button
      >
      {#if error}
        <span class="np-test-result">{error}</span>
      {:else if restartPending}
        <span class="np-test-result"
          >Saved. Restart RadiodioDJ for it to take effect.</span
        >
      {/if}
    </div>
  {/if}

  <h5 class="tuning-group-title">Now</h5>
  <p class="settings-section-desc" role="status">
    {#if running === "standalone"}
      Not shared.
    {:else}
      Running as <strong>{roleLabel(running).toLowerCase()}</strong>.
      {saved.status.message}
      {#if saved.status.message === ""}
        Waiting for the hub.
      {:else if saved.status.ok}
        The hub answered {formatAgo(saved.status.reachedAt ?? Date.now())}.
      {:else if saved.status.reachedAt !== null}
        It last answered {formatAgo(saved.status.reachedAt)}.
      {:else}
        It has not answered since RadiodioDJ started.
      {/if}
    {/if}
  </p>
</div>
