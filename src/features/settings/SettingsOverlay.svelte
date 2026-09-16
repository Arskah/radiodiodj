<script lang="ts">
  import { api } from "../../shared/api";
  import { app } from "../../shared/state.svelte";
  import LibraryHealth from "../health/LibraryHealth.svelte";
  import type {
    ContentType,
    DeviceInfo,
    DeviceRef,
    NowPlayingConfig,
    TuningConfig,
  } from "../../shared/types";

  const sections: { type: ContentType; label: string }[] = [
    { type: "music", label: "Music" },
    { type: "commercial", label: "Commercials" },
    { type: "jingle", label: "Jingles" },
  ];

  const MIB = 1024 * 1024;
  // Editable draft of the tuning config. Synced from `app.tuning` whenever the
  // overlay opens; each edit persists via `app.saveTuning`, then re-syncs so the
  // backend's clamped values are reflected in the inputs.
  let tuning = $state<TuningConfig>($state.snapshot(app.tuning));
  let mainDeviceChanged = $state(false);
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
    if (app.settingsOpen) {
      void app.loadAudioConfig();
      void loadNowPlayingConfig();
      tuning = $state.snapshot(app.tuning);
      mainDeviceChanged = false;
    }
  });

  // Persist the current draft, then adopt the backend's clamped result so the
  // inputs snap to any coerced values.
  async function saveTuning(): Promise<void> {
    await app.saveTuning($state.snapshot(tuning));
    tuning = $state.snapshot(app.tuning);
  }

  // Parse a number input, ignoring empty/NaN so a mid-edit blank doesn't wipe
  // the field; the min-clamp is enforced by the backend on save.
  function numInput(e: Event, apply: (v: number) => void): void {
    const v = Number((e.currentTarget as HTMLInputElement).value);
    if (Number.isFinite(v)) apply(v);
  }

  // Parse a comma/space separated list of positive integers (backoff schedules).
  function listInput(e: Event, apply: (v: number[]) => void): void {
    const parsed = (e.currentTarget as HTMLInputElement).value
      .split(/[,\s]+/)
      .map((s) => Number(s))
      .filter((n) => Number.isFinite(n) && n > 0);
    if (parsed.length > 0) apply(parsed);
  }

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

  function deviceKey(d: DeviceInfo | DeviceRef | null): string {
    return d ? `${d.name}|${d.description}` : "";
  }

  function findDevice(key: string): DeviceInfo | null {
    return app.audioDevices.find((d) => deviceKey(d) === key) ?? null;
  }

  async function onMainDeviceChange(e: Event): Promise<void> {
    const key = (e.currentTarget as HTMLSelectElement).value;
    if (key === "") {
      await app.setMainDeviceConfig(null);
    } else {
      const d = findDevice(key);
      if (d)
        await app.setMainDeviceConfig({
          name: d.name,
          description: d.description,
        });
    }
    mainDeviceChanged = true;
  }

  async function onCueDeviceChange(e: Event): Promise<void> {
    const key = (e.currentTarget as HTMLSelectElement).value;
    if (key === "") {
      await app.setCueDeviceConfig(null);
    } else {
      const d = findDevice(key);
      if (d)
        await app.setCueDeviceConfig({
          name: d.name,
          description: d.description,
        });
    }
  }

  async function onScan(): Promise<void> {
    await app.scan();
    app.settingsOpen = false;
  }
</script>

<div id="settings-overlay" class:hidden={!app.settingsOpen}>
  <div id="settings-modal">
    <div id="settings-modal-header">
      <span class="settings-modal-title">
        <span class="material-symbols-outlined" aria-hidden="true"
          >settings</span
        >
        Preferences
      </span>
      <button
        id="btn-close-settings-x"
        title="Close"
        aria-label="Close settings"
        onclick={() => (app.settingsOpen = false)}
      >
        <span class="material-symbols-outlined">close</span>
      </button>
    </div>

    <div id="settings-body">
      <div id="settings-sidebar" role="tablist" aria-label="Settings sections">
        <button
          class="settings-tab"
          class:active={app.settingsTab === "audio"}
          role="tab"
          aria-selected={app.settingsTab === "audio"}
          onclick={() => (app.settingsTab = "audio")}
        >
          <span class="material-symbols-outlined">volume_up</span>
          Audio Output
        </button>
        <button
          class="settings-tab"
          class:active={app.settingsTab === "library"}
          role="tab"
          aria-selected={app.settingsTab === "library"}
          onclick={() => (app.settingsTab = "library")}
        >
          <span class="material-symbols-outlined">library_music</span>
          Library
          {#if app.healthAttention > 0}
            <span
              class="attention-badge"
              aria-label={`${app.healthAttention} need attention`}
              >{app.healthAttention}</span
            >
          {/if}
        </button>
        <button
          class="settings-tab"
          class:active={app.settingsTab === "now-playing"}
          role="tab"
          aria-selected={app.settingsTab === "now-playing"}
          onclick={() => (app.settingsTab = "now-playing")}
        >
          <span class="material-symbols-outlined">rss_feed</span>
          Now Playing
        </button>
        <button
          class="settings-tab"
          class:active={app.settingsTab === "advanced"}
          role="tab"
          aria-selected={app.settingsTab === "advanced"}
          onclick={() => (app.settingsTab = "advanced")}
        >
          <span class="material-symbols-outlined">tune</span>
          Advanced
        </button>
      </div>

      <div id="settings-content">
        {#if app.settingsTab === "audio"}
          <div class="settings-section">
            <h4>Audio Configuration</h4>
            <p class="settings-section-desc">
              Configure your signal chain for low-latency broadcast performance.
            </p>
            {#if app.audioDevices.length === 0}
              <div class="empty">
                <span class="empty-icon"
                  ><span class="material-symbols-outlined">volume_off</span
                  ></span
                >
                <span class="empty-title">No Output Devices</span>
                <span class="empty-body"
                  >No audio output devices were detected on this system.</span
                >
              </div>
            {:else}
              <div class="device-row">
                <label for="main-device">Master Output Device</label>
                <select
                  id="main-device"
                  value={deviceKey(app.mainDevice)}
                  onchange={onMainDeviceChange}
                >
                  <option value="">System default</option>
                  {#each app.audioDevices as d (deviceKey(d))}
                    <option value={deviceKey(d)}>
                      {d.description}{d.isDefault ? " (default)" : ""}
                    </option>
                  {/each}
                </select>
                {#if mainDeviceChanged}
                  <div class="hint">
                    Restart required to apply main-device change.
                  </div>
                {/if}
              </div>

              <div class="device-row">
                <label for="cue-device">Cue / Headphones Output</label>
                <select
                  id="cue-device"
                  value={deviceKey(app.cueDevice)}
                  onchange={onCueDeviceChange}
                >
                  <option value="">Disabled</option>
                  {#each app.audioDevices as d (deviceKey(d))}
                    <option value={deviceKey(d)}>
                      {d.description}{d.isDefault ? " (default)" : ""}
                    </option>
                  {/each}
                </select>
                <div class="hint">
                  Pick a different device than main for headphone preview.
                </div>
              </div>
            {/if}
          </div>
        {:else if app.settingsTab === "library"}
          <div class="settings-section">
            <h4>Library</h4>
            <p class="settings-section-desc">
              Where your media lives, and what needs attention in it: tracks
              whose files are gone, copies of the same recording, and changes on
              disk that no scan has picked up yet. Nothing here changes an audio
              file — delete an unwanted copy in the file manager, then scan.
            </p>
            <div id="paths-list">
              {#each sections as { type, label } (type)}
                <div class="path-section">
                  <div class="path-section-header">
                    <span>{label}</span>
                    <button
                      class="btn-add-section"
                      title="Add {label} folder"
                      onclick={() => app.addPath(type)}
                    >
                      <span class="material-symbols-outlined">add_circle</span>
                      Add Directory
                    </button>
                  </div>
                  {#if (app.libraryPaths[type] ?? []).length === 0}
                    <div class="path-empty">
                      No {label.toLowerCase()} directories defined. Click "Add Directory"
                      to begin.
                    </div>
                  {:else}
                    {#each app.libraryPaths[type] as p (p)}
                      <div class="path-row">
                        <span class="material-symbols-outlined">folder</span>
                        <span class="path-text">{p}</span>
                        <button
                          class="btn-remove"
                          title="Remove"
                          aria-label="Remove directory"
                          onclick={() => app.removePath(type, p)}
                        >
                          <span class="material-symbols-outlined">close</span>
                        </button>
                      </div>
                    {/each}
                  {/if}
                </div>
              {/each}
            </div>
            <div class="settings-row">
              <button
                id="btn-scan-now"
                class="btn-scan-now"
                title="Scan all configured paths"
                onclick={onScan}
              >
                <span class="material-symbols-outlined">sync</span>
                Scan Library Now
              </button>
            </div>
            <LibraryHealth />
          </div>
        {:else if app.settingsTab === "now-playing"}
          <div class="settings-section">
            <h4>Now Playing Metadata</h4>
            <p class="settings-section-desc">
              Expose the currently playing track to external consumers via
              outbound webhook and/or local files. Updates fire on track-start
              and on stop.
            </p>

            <div class="np-group" class:disabled={!nowPlaying.webhookEnabled}>
              <div class="np-group-header">
                <span class="material-symbols-outlined" aria-hidden="true"
                  >webhook</span
                >
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
                <label class="np-field-label" for="np-webhook-url"
                  >Target URL</label
                >
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
                        webhookUrl:
                          (e.currentTarget as HTMLInputElement).value || null,
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
                <span class="material-symbols-outlined" aria-hidden="true"
                  >save</span
                >
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
                <label class="np-field-label" for="np-file-dir"
                  >Export Directory</label
                >
                <div class="np-input-wrap">
                  <span class="material-symbols-outlined">folder_open</span>
                  <span id="np-file-dir" class="np-file-dir"
                    >{nowPlaying.fileDir ??
                      "(app data dir / now-playing)"}</span
                  >
                  <button class="np-browse" onclick={pickFileDir}>
                    <span class="material-symbols-outlined">search</span>
                    Browse
                  </button>
                  {#if nowPlaying.fileDir}
                    <button class="np-browse" onclick={clearFileDir}
                      >Reset</button
                    >
                  {/if}
                </div>
              </div>
              <div class="np-file-hint">
                Writes <code>now_playing.txt</code> and
                <code>now_playing.json</code> atomically. TXT is truncated on stop;
                JSON keeps the Stopped event payload.
              </div>
            </div>
          </div>
        {:else}
          <div class="settings-section settings-section--tuning">
            <h4>Advanced Tuning</h4>
            <p class="settings-section-desc">
              Fine-tune playlist rotation, buffering, and network resilience.
              Out-of-range values are clamped on save.
            </p>

            <h5 class="tuning-group-title">Library</h5>
            <div class="device-row">
              <label for="tune-check-interval"
                >Library check interval (minutes)</label
              >
              <input
                id="tune-check-interval"
                type="number"
                min="0"
                value={tuning.library.checkIntervalMin}
                oninput={(e) =>
                  numInput(e, (v) => (tuning.library.checkIntervalMin = v))}
                onchange={saveTuning}
              />
              <div class="hint">
                How often to look for files added, changed or removed since the
                last scan. Reads no audio, and never changes the library. 0
                turns it off.
              </div>
            </div>

            <h5 class="tuning-group-title">Interleave</h5>
            <div class="device-row">
              <label for="tune-jingle-every">Jingle every N tracks</label>
              <input
                id="tune-jingle-every"
                type="number"
                min="0"
                value={tuning.interleave.jingleEvery}
                oninput={(e) =>
                  numInput(e, (v) => (tuning.interleave.jingleEvery = v))}
                onchange={saveTuning}
              />
              <div class="hint">
                One jingle after this many music tracks. 0 disables jingles.
              </div>
            </div>
            <div class="device-row">
              <label for="tune-commercial-every"
                >Commercial every N tracks</label
              >
              <input
                id="tune-commercial-every"
                type="number"
                min="0"
                value={tuning.interleave.commercialEvery}
                oninput={(e) =>
                  numInput(e, (v) => (tuning.interleave.commercialEvery = v))}
                onchange={saveTuning}
              />
              <div class="hint">
                One commercial break after this many music tracks. 0 disables
                commercials.
              </div>
            </div>
            <div class="device-row">
              <label for="tune-bucket-mult">Commercial bucket multiplier</label>
              <input
                id="tune-bucket-mult"
                type="number"
                min="1"
                value={tuning.interleave.commercialBucketMultiplier}
                oninput={(e) =>
                  numInput(
                    e,
                    (v) => (tuning.interleave.commercialBucketMultiplier = v),
                  )}
                onchange={saveTuning}
              />
              <div class="hint">
                Commercials per break scale with playlist length times this
                factor. Higher = more ads per break.
              </div>
            </div>
            <div class="device-row">
              <label for="tune-bucket-min">Commercial bucket minimum</label>
              <input
                id="tune-bucket-min"
                type="number"
                min="0"
                value={tuning.interleave.commercialBucketMin}
                oninput={(e) =>
                  numInput(
                    e,
                    (v) => (tuning.interleave.commercialBucketMin = v),
                  )}
                onchange={saveTuning}
              />
              <div class="hint">
                Floor on commercials per break, regardless of playlist length. 0
                disables the floor.
              </div>
            </div>

            <h5 class="tuning-group-title">Auto-playlist</h5>
            <div class="device-row">
              <label for="tune-buffer">Buffer (tracks kept queued)</label>
              <input
                id="tune-buffer"
                type="number"
                min="1"
                value={tuning.autoPlaylist.autoPlaylistBuffer}
                oninput={(e) =>
                  numInput(
                    e,
                    (v) => (tuning.autoPlaylist.autoPlaylistBuffer = v),
                  )}
                onchange={saveTuning}
              />
              <div class="hint">
                Target queue length the auto-playlist tops up to. Minimum 1.
              </div>
            </div>
            <div class="device-row">
              <label for="tune-threshold">Refill threshold</label>
              <input
                id="tune-threshold"
                type="number"
                min="1"
                value={tuning.autoPlaylist.autoPlaylistThreshold}
                oninput={(e) =>
                  numInput(
                    e,
                    (v) => (tuning.autoPlaylist.autoPlaylistThreshold = v),
                  )}
                onchange={saveTuning}
              />
              <div class="hint">
                Refill kicks in when the queue drops to this many tracks.
                Clamped to at most the buffer size.
              </div>
            </div>
            <div class="device-row">
              <label for="tune-history">History cap</label>
              <input
                id="tune-history"
                type="number"
                min="1"
                value={tuning.autoPlaylist.historyCap}
                oninput={(e) =>
                  numInput(e, (v) => (tuning.autoPlaylist.historyCap = v))}
                onchange={saveTuning}
              />
              <div class="hint">
                Recently played tracks remembered to avoid quick repeats.
                Minimum 1.
              </div>
            </div>
            <div class="device-row">
              <label for="tune-save-throttle">Session save throttle (ms)</label>
              <input
                id="tune-save-throttle"
                type="number"
                min="0"
                value={tuning.autoPlaylist.sessionSaveThrottleMs}
                oninput={(e) =>
                  numInput(
                    e,
                    (v) => (tuning.autoPlaylist.sessionSaveThrottleMs = v),
                  )}
                onchange={saveTuning}
              />
              <div class="hint">
                Minimum gap between session writes to disk. 0 saves on every
                change (more disk I/O).
              </div>
            </div>
            <div class="device-row">
              <label for="tune-net-backoffs">Network retry backoffs (ms)</label>
              <input
                id="tune-net-backoffs"
                type="text"
                value={tuning.autoPlaylist.netRetryBackoffsMs.join(", ")}
                oninput={(e) =>
                  listInput(
                    e,
                    (v) => (tuning.autoPlaylist.netRetryBackoffsMs = v),
                  )}
                onchange={saveTuning}
              />
              <div class="hint">
                Comma-separated wait times between refill retries. The last
                value repeats until recovery.
              </div>
            </div>

            <h5 class="tuning-group-title">Network &amp; cache</h5>
            <div class="device-row">
              <label for="tune-cache">Prefetch cache size (MiB)</label>
              <input
                id="tune-cache"
                type="number"
                min="16"
                value={Math.round(tuning.cache.maxCacheBytes / MIB)}
                oninput={(e) =>
                  numInput(
                    e,
                    (v) => (tuning.cache.maxCacheBytes = Math.round(v * MIB)),
                  )}
                onchange={saveTuning}
              />
              <div class="hint">
                RAM budget for prefetched track files. Minimum 16 MiB.
              </div>
            </div>
            <div class="device-row">
              <label for="tune-watchdog">Read watchdog timeout (ms)</label>
              <input
                id="tune-watchdog"
                type="number"
                min="1"
                value={tuning.player.readWatchdogTimeoutMs}
                oninput={(e) =>
                  numInput(e, (v) => (tuning.player.readWatchdogTimeoutMs = v))}
                onchange={saveTuning}
              />
              <div class="hint">
                A read stalled longer than this counts as a network hiccup and
                triggers recovery.
              </div>
            </div>
            <div class="device-row">
              <label for="tune-open-retry"
                >Output open-retry interval (ms)</label
              >
              <input
                id="tune-open-retry"
                type="number"
                min="1"
                value={tuning.player.openRetryIntervalMs}
                oninput={(e) =>
                  numInput(e, (v) => (tuning.player.openRetryIntervalMs = v))}
                onchange={saveTuning}
              />
              <div class="hint">
                Wait between attempts to reopen the audio output device.
              </div>
            </div>
            <div class="device-row">
              <label for="tune-read-backoffs">Read retry backoffs (ms)</label>
              <input
                id="tune-read-backoffs"
                type="text"
                value={tuning.player.readRetryBackoffsMs.join(", ")}
                oninput={(e) =>
                  listInput(e, (v) => (tuning.player.readRetryBackoffsMs = v))}
                onchange={saveTuning}
              />
              <div class="hint">
                Comma-separated wait times between file-read retries. The last
                value repeats until recovery.
              </div>
            </div>
            <div class="hint">
              Cache and player settings apply on next restart.
            </div>
          </div>
        {/if}
      </div>
    </div>

    <div id="settings-footer">
      <span class="settings-footer-info">
        <span class="material-symbols-outlined" aria-hidden="true">info</span>
        All changes are saved automatically
      </span>
      <div id="settings-actions">
        <button
          id="btn-close-settings"
          onclick={() => (app.settingsOpen = false)}>Close</button
        >
      </div>
    </div>
  </div>
</div>
