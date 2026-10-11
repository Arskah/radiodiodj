<script lang="ts">
  import { app } from "../../shared/state.svelte";
  import { formatAgo } from "../../shared/health";
  import { replacesLibrary, roleLabel } from "../../shared/sharedLibrary";
  import { api } from "../../shared/api";
  import type { CaCertificate, LibraryRole } from "../../shared/types";

  const roles: LibraryRole[] = ["standalone", "owner", "studio"];

  // A draft, saved by its own button: a role is not something to change with
  // a stray click, and nothing here applies before a restart anyway.
  let role = $state<LibraryRole>("standalone");
  let url = $state("");
  let machineName = $state("");
  let allowUnencrypted = $state(false);
  let directTls = $state(false);
  let ca = $state<CaCertificate | null>(null);
  let showUrl = $state(false);
  let confirming = $state(false);
  let saving = $state(false);
  let error = $state<string | null>(null);

  const saved = $derived(app.sharedLibrary);
  const running = $derived(saved.status.role);
  const dirty = $derived(
    role !== saved.role ||
      url.trim() !== (saved.url ?? "") ||
      machineName.trim() !== (saved.machineName ?? "") ||
      allowUnencrypted !== saved.allowUnencrypted ||
      directTls !== saved.directTls ||
      (ca?.pem ?? null) !== (saved.caCertificate?.pem ?? null),
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
    allowUnencrypted = app.sharedLibrary.allowUnencrypted;
    directTls = app.sharedLibrary.directTls;
    ca = app.sharedLibrary.caCertificate;
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
      await app.saveSharedLibrary({
        role,
        url: url.trim() || null,
        machineName: machineName.trim() || null,
        allowUnencrypted,
        directTls,
        caCertificate: ca?.pem ?? null,
      });
      adopt();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      saving = false;
    }
  }

  async function chooseCa(): Promise<void> {
    error = null;
    try {
      const path = await api.pickCertificateFile();
      if (path) ca = await api.readCaCertificate(path);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
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

    <h5 class="tuning-group-title">Connection</h5>
    <p class="settings-section-desc">
      The connection to the hub is encrypted, and the hub has to prove who it is
      with a certificate for the host name in its address. A certificate from a
      public authority needs nothing here.
    </p>
    <div class="np-field">
      <span class="np-field-label">Hub's own certificate authority</span>
      <div class="np-input-wrap">
        <span class="material-symbols-outlined">verified_user</span>
        <span id="shared-ca" class="np-file-dir">
          {#if ca}
            {ca.summary.subject}{ca.summary.count > 1
              ? ` and ${ca.summary.count - 1} more`
              : ""} — expires {new Date(
              ca.summary.expiresAt,
            ).toLocaleDateString()}
          {:else}
            None — only public authorities are trusted
          {/if}
        </span>
        <button class="np-browse" onclick={chooseCa}>
          <span class="material-symbols-outlined">search</span>
          Choose
        </button>
        {#if ca}
          <button class="np-browse" onclick={() => (ca = null)}>Remove</button>
        {/if}
      </div>
      <div class="np-file-hint">
        For a hub whose certificate comes from an authority of its own, as a
        database inside a cluster usually has. A <code>.pem</code> or
        <code>.crt</code> file; it is trusted for the hub only, beside the ones this
        computer already trusts.
      </div>
    </div>
    <div class="np-group" class:disabled={!directTls}>
      <div class="np-group-header">
        <span class="material-symbols-outlined" aria-hidden="true">lan</span>
        <span class="np-group-title">Encryption ends at a proxy</span>
        <label class="np-toggle" title="Start with the TLS handshake">
          <input
            id="shared-direct-tls"
            type="checkbox"
            bind:checked={directTls}
          />
          <span class="np-toggle-track"></span>
        </label>
      </div>
      <p class="settings-section-desc">
        Turn on when a load balancer or ingress in front of the hub holds the
        certificate. The connection then opens with the TLS handshake, which
        such a proxy needs to see first. Leave off when the database holds its
        own certificate, unless it is PostgreSQL 17 or later.
      </p>
    </div>
    <div class="np-group" class:disabled={!allowUnencrypted}>
      <div class="np-group-header">
        <span class="material-symbols-outlined" aria-hidden="true"
          >lock_open</span
        >
        <span class="np-group-title">Allow an unencrypted connection</span>
        <label class="np-toggle" title="Allow an unencrypted connection">
          <input
            id="shared-allow-unencrypted"
            type="checkbox"
            disabled={directTls}
            bind:checked={allowUnencrypted}
          />
          <span class="np-toggle-track"></span>
        </label>
      </div>
      <p class="settings-section-desc">
        Only for a hub on a private network or behind a tunnel. With this on, a
        hub that offers no encryption is used anyway, and the password and the
        library cross the network readable. Encryption is still used wherever
        the hub offers it.
      </p>
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
        The hub answered {formatAgo(
          saved.status.reachedAt ?? Date.now(),
        )}{#if saved.status.encrypted === true}, over an encrypted connection.{:else if saved.status.encrypted === false},
          over a connection that is <strong>not encrypted</strong>.{:else}.{/if}
      {:else if saved.status.reachedAt !== null}
        It last answered {formatAgo(saved.status.reachedAt)}.
      {:else}
        It has not answered since RadiodioDJ started.
      {/if}
    {/if}
  </p>
</div>
