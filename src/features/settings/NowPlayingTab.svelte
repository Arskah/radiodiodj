<script lang="ts">
  import { api } from "../../shared/api";
  import { app } from "../../shared/state.svelte";
  import type { NowPlayingConfig } from "../../shared/types";

  let nowPlaying = $state<NowPlayingConfig>({
    webhookUrl: null,
    webhookSecret: null,
    fileDir: null,
    fileEnabled: true,
    webhookEnabled: true,
  });
  let testResult = $state<string | null>(null);
  let testing = $state(false);
  let showSecret = $state(false);

  $effect(() => {
    if (app.settingsOpen) void loadNowPlayingConfig();
  });

  async function loadNowPlayingConfig(): Promise<void> {
    nowPlaying = await api.getNowPlayingConfig();
  }

  async function saveNowPlaying(): Promise<void> {
    await api.setNowPlayingConfig(nowPlaying);
  }

  async function pickFileDir(): Promise<void> {
    const dir = await api.pickDirectory();
    if (dir) {
      nowPlaying = { ...nowPlaying, fileDir: dir };
      await saveNowPlaying();
    }
  }

  function clearFileDir(): void {
    nowPlaying = { ...nowPlaying, fileDir: null };
    void saveNowPlaying();
  }

  async function runTestWebhook(): Promise<void> {
    testing = true;
    testResult = null;
    try {
      const status = await api.testNowPlayingWebhook();
      testResult = `HTTP ${status}`;
    } catch (e) {
      testResult = `Error: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      testing = false;
    }
  }
</script>

<div class="settings-section">
  <h4>Now Playing Metadata</h4>
  <p class="settings-section-desc">
    Expose the currently playing track to external consumers via outbound
    webhook and/or local files. Updates fire on track-start and on stop.
  </p>
  <div class="np-group" class:disabled={!nowPlaying.webhookEnabled}>
    <div class="np-group-header">
      <span class="material-symbols-outlined" aria-hidden="true">webhook</span>
      <span class="np-group-title">Webhook Export</span>
      <label class="np-toggle" title="Enable webhook export">
        <input
          type="checkbox"
          bind:checked={nowPlaying.webhookEnabled}
          onchange={saveNowPlaying}
        />
        <span class="np-toggle-track"></span>
      </label>
    </div>
    <div class="np-field">
      <label class="np-field-label" for="np-webhook-url">Target URL</label>
      <div class="np-input-wrap">
        <span class="material-symbols-outlined">link</span>
        <input
          id="np-webhook-url"
          class="np-input"
          type="url"
          placeholder="https://example.com/now-playing"
          value={nowPlaying.webhookUrl ?? ""}
          oninput={(e) =>
            (nowPlaying = {
              ...nowPlaying,
              webhookUrl: (e.currentTarget as HTMLInputElement).value || null,
            })}
          onchange={saveNowPlaying}
        />
      </div>
    </div>
    <div class="np-field">
      <label class="np-field-label" for="np-webhook-secret"
        >HMAC Secret (optional)</label
      >
      <div class="np-input-wrap">
        <span class="material-symbols-outlined">key</span>
        <input
          id="np-webhook-secret"
          class="np-input mono"
          type={showSecret ? "text" : "password"}
          placeholder="optional"
          autocapitalize="off"
          autocorrect="off"
          autocomplete="off"
          spellcheck="false"
          value={nowPlaying.webhookSecret ?? ""}
          oninput={(e) =>
            (nowPlaying = {
              ...nowPlaying,
              webhookSecret:
                (e.currentTarget as HTMLInputElement).value || null,
            })}
          onchange={saveNowPlaying}
        />
        <button
          type="button"
          class="np-eye"
          title={showSecret ? "Hide secret" : "Show secret"}
          aria-label={showSecret ? "Hide secret" : "Show secret"}
          onclick={() => (showSecret = !showSecret)}
        >
          <span class="material-symbols-outlined"
            >{showSecret ? "visibility_off" : "visibility"}</span
          >
        </button>
      </div>
    </div>
    <div class="np-action-row">
      <button
        class="btn-scan-now"
        onclick={runTestWebhook}
        disabled={testing || !nowPlaying.webhookUrl}
        >{testing ? "Testing…" : "Test webhook"}</button
      >
      {#if testResult}
        <span class="np-test-result">{testResult}</span>
      {/if}
    </div>
  </div>
  <div class="np-group" class:disabled={!nowPlaying.fileEnabled}>
    <div class="np-group-header">
      <span class="material-symbols-outlined" aria-hidden="true">save</span>
      <span class="np-group-title">Local File Export</span>
      <label class="np-toggle" title="Enable file export">
        <input
          type="checkbox"
          bind:checked={nowPlaying.fileEnabled}
          onchange={saveNowPlaying}
        />
        <span class="np-toggle-track"></span>
      </label>
    </div>
    <div class="np-field">
      <label class="np-field-label" for="np-file-dir">Export Directory</label>
      <div class="np-input-wrap">
        <span class="material-symbols-outlined">folder_open</span>
        <span id="np-file-dir" class="np-file-dir"
          >{nowPlaying.fileDir ?? "(app data dir / now-playing)"}</span
        >
        <button class="np-browse" onclick={pickFileDir}>
          <span class="material-symbols-outlined">search</span>
          Browse
        </button>
        {#if nowPlaying.fileDir}
          <button class="np-browse" onclick={clearFileDir}>Reset</button>
        {/if}
      </div>
    </div>
    <div class="np-file-hint">
      Writes <code>now_playing.txt</code> and
      <code>now_playing.json</code> atomically. TXT is truncated on stop; JSON keeps
      the Stopped event payload.
    </div>
  </div>
</div>
