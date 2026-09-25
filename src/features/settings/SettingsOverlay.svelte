<script lang="ts">
  import { api } from "../../shared/api";
  import { app } from "../../shared/state.svelte";
  import LibraryHealth from "../health/LibraryHealth.svelte";
  import type {
    ContentType,
    DeviceInfo,
    DeviceRef,
    ImageSlot,
    NowPlayingConfig,
    ReplayGainMode,
    TuningConfig,
  } from "../../shared/types";
  import { APP_NAME } from "../../shared/appName";

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
  let recalcResult = $state<string | null>(null);
  let recalculating = $state(false);
  // Clicking Recalculate now is what blurs a threshold input, so the save and
  // the recalculation are two handlers in one gesture, and the recalculation
  // reads the thresholds on the backend. It waits for this.
  let tuningSave: Promise<unknown> | null = null;
  // A scan rewrites the rows a recalculation reads, so the backend refuses
  // one while it runs. Say so before the press rather than after.
  const scanning = $derived(app.scanStatus.status === "running");
  let showSecret = $state(false);
  let newPassword = $state("");
  let confirmPassword = $state("");
  let passwordFormOpen = $state(false);
  let confirmingRemove = $state(false);
  let passwordBusy = $state(false);
  let passwordError = $state<string | null>(null);

  $effect(() => {
    if (app.settingsOpen) {
      newPassword = "";
      confirmPassword = "";
      passwordFormOpen = false;
      confirmingRemove = false;
      passwordError = null;
      recalcResult = null;
      void app.loadAudioConfig();
      void loadNowPlayingConfig();
      // Covers "I dropped a folder in, then came here to look for it".
      void app.loadThemes();
      tuning = $state.snapshot(app.tuning);
      mainDeviceChanged = false;
    }
  });

  // Draft of the station name, resynced whenever the backend hands back a new
  // appearance — it trims and caps, so the input must adopt what was stored.
  let stationName = $state("");
  $effect(() => {
    stationName = app.appearance?.stationName ?? "";
  });

  async function saveStationName(): Promise<void> {
    const next = stationName.trim();
    await app.setStationName(next === "" ? null : next);
  }

  async function pickImage(slot: ImageSlot): Promise<void> {
    const path = await api.pickImageFile();
    if (path) await app.setStationImage(slot, path);
  }

  // The two slots differ in shape, so one image cannot serve both: the toolbar
  // wants wide and short, the record label is cropped to a circle.
  const imageSlots = $derived([
    {
      slot: "logo" as ImageSlot,
      label: "Toolbar logo",
      src: app.appearance?.logo ?? null,
      owned: app.appearance?.logo != null,
      round: false,
      hint: "Replaces the station name in the toolbar. Wide and short suits it best.",
    },
    {
      slot: "label" as ImageSlot,
      label: "Record label",
      src: app.appearance?.label ?? null,
      owned: app.appearance?.label != null,
      round: true,
      hint: "The centre of the deck's vinyl when a track has no cover art. Square, and keep anything important inside the circle — cover art wins when a track has it.",
    },
  ]);

  // Selecting a theme applies it immediately — the running app is the preview,
  // and reverting is picking the previous entry.
  async function pickTheme(themeId: string): Promise<void> {
    await app.setTheme(themeId);
  }

  // Re-enumerates and re-resolves, so editing the active theme and pressing
  // this repaints. A theme that has become invalid leaves the colours alone.
  async function reload(): Promise<void> {
    await app.reloadThemes();
  }

  // Persist the current draft, then adopt the backend's clamped result so the
  // inputs snap to any coerced values.
  /**
   * What a tuning field showed when the operator entered it. A field they
   * clear never reaches the store — see `numInput` — so the `value` binding
   * has nothing to re-write on save and the box would sit empty until the
   * overlay is reopened. This is what goes back into it.
   */
  const shownOnFocus = new WeakMap<HTMLInputElement, string>();

  function rememberField(e: FocusEvent): void {
    const el = e.target;
    if (el instanceof HTMLInputElement && el.type === "number") {
      shownOnFocus.set(el, el.value);
    }
  }

  async function saveTuning(e?: Event): Promise<void> {
    const el = e?.target;
    if (
      el instanceof HTMLInputElement &&
      el.type === "number" &&
      el.value.trim() === ""
    ) {
      el.value = shownOnFocus.get(el) ?? el.value;
    }
    const save = app.saveTuning($state.snapshot(tuning));
    tuningSave = save;
    try {
      await save;
      tuning = $state.snapshot(app.tuning);
    } finally {
      if (tuningSave === save) tuningSave = null;
    }
  }

  /**
   * Edit one of the automatic-cue thresholds. The result line under the button
   * names the levels the last recalculation ran at, so a fresh edit makes it a
   * lie — drop it on the first keystroke.
   */
  function thresholdInput(e: Event, apply: (v: number) => void): void {
    recalcResult = null;
    numInput(e, apply);
  }

  /**
   * Apply the thresholds above to material already analysed. Says what it did
   * rather than how far it got: the rows it re-derived are done when this
   * resolves, and the rows it queued are the analysis bar's business.
   *
   * The levels come back from `app.tuning` rather than the inputs, because the
   * backend rounds and clamps them — the line has to name what was applied, not
   * what was typed.
   */
  async function recalculateAutoCue(): Promise<void> {
    recalculating = true;
    recalcResult = null;
    try {
      await tuningSave;
      const done = await app.recalculateAutoCue();
      const { silenceDbfs, segueDbfs } = app.tuning.autoCue;
      const at = `at ${silenceDbfs} / ${segueDbfs} dBFS`;
      const parts = [`${done.updated.toLocaleString()} re-derived ${at}`];
      if (done.queued > 0) {
        parts.push(`${done.queued.toLocaleString()} queued for analysis`);
      }
      if (done.manual > 0) {
        parts.push(`${done.manual.toLocaleString()} radio edits left alone`);
      }
      recalcResult = parts.join(" · ");
    } catch (err) {
      recalcResult = err instanceof Error ? err.message : String(err);
    } finally {
      recalculating = false;
    }
  }

  // Parse a number input, ignoring empty/NaN so a mid-edit blank doesn't wipe
  // the field; the min-clamp is enforced by the backend on save.
  function numInput(e: Event, apply: (v: number) => void): void {
    // `Number("")` is 0, which is finite: without the blank check a cleared
    // field reads as a deliberate zero on blur. Harmless where 0 clamps toward
    // a floor, not on a range that excludes it — the dBFS levels would clamp
    // to their *ceiling* and trim every later analysis to nothing.
    const raw = (e.currentTarget as HTMLInputElement).value.trim();
    if (raw === "") return;
    const v = Number(raw);
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

  async function savePassword(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    if (newPassword === "") {
      passwordError = "Enter a password";
      return;
    }
    if (newPassword !== confirmPassword) {
      passwordError = "The passwords do not match";
      return;
    }
    passwordBusy = true;
    passwordError = null;
    try {
      await app.setAdminPassword(newPassword);
      newPassword = "";
      confirmPassword = "";
      passwordFormOpen = false;
    } catch (err) {
      passwordError = err instanceof Error ? err.message : String(err);
    } finally {
      passwordBusy = false;
    }
  }

  async function removePassword(): Promise<void> {
    passwordBusy = true;
    passwordError = null;
    try {
      await app.clearAdminPassword();
      confirmingRemove = false;
    } catch (err) {
      passwordError = err instanceof Error ? err.message : String(err);
    } finally {
      passwordBusy = false;
    }
  }

  function saveIdleLockMin(e: Event): void {
    const v = Number((e.currentTarget as HTMLInputElement).value);
    if (!Number.isFinite(v)) return;
    void app.setIdleLockMin(Math.round(v)).catch((err) => {
      passwordError = err instanceof Error ? err.message : String(err);
    });
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
          class:active={app.settingsTab === "playlist"}
          role="tab"
          aria-selected={app.settingsTab === "playlist"}
          onclick={() => (app.settingsTab = "playlist")}
        >
          <span class="material-symbols-outlined">queue_music</span>
          Playlist
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
          class:active={app.settingsTab === "appearance"}
          role="tab"
          aria-selected={app.settingsTab === "appearance"}
          onclick={() => (app.settingsTab = "appearance")}
        >
          <span class="material-symbols-outlined">palette</span>
          Appearance
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
                <span class="select-wrap">
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
                </span>
                {#if mainDeviceChanged}
                  <div class="hint">
                    Restart required to apply main-device change.
                  </div>
                {/if}
              </div>

              <div class="device-row">
                <label for="cue-device">Cue / Headphones Output</label>
                <span class="select-wrap">
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
                </span>
                <div class="hint">
                  Pick a different device than main for headphone preview.
                </div>
              </div>
            {/if}

            <div class="device-row">
              <label for="setting-replay-gain">Track levelling</label>
              <span class="select-wrap">
                <select
                  id="setting-replay-gain"
                  value={tuning.player.replayGain}
                  onchange={(e) => {
                    tuning.player.replayGain = e.currentTarget
                      .value as ReplayGainMode;
                    saveTuning();
                  }}
                >
                  <option value="track">Level every track</option>
                  <option value="off">Play as mastered</option>
                </select>
              </span>
              <div class="hint">
                Brings every track to the same loudness, so a quiet song does
                not disappear after a loud one and crossfades mix at the levels
                you hear. Applies to both outputs. Measured during the waveform
                pass — a track stays unlevelled until that reaches it.
              </div>
            </div>
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
            <div class="np-group" class:disabled={!tuning.library.writeTags}>
              <div class="np-group-header">
                <span class="material-symbols-outlined" aria-hidden="true"
                  >edit_document</span
                >
                <span class="np-group-title">Write edits to file tags</span>
                <label class="np-toggle" title="Write metadata edits to files">
                  <input
                    id="setting-write-tags"
                    type="checkbox"
                    bind:checked={tuning.library.writeTags}
                    onchange={saveTuning}
                  />
                  <span class="np-toggle-track"></span>
                </label>
              </div>
              <p class="settings-section-desc">
                Metadata edits are always kept in the library. With this on,
                they are also written into the audio file, so they travel with
                it. This modifies files on your library paths, network shares
                included. Failed writes are listed under Library health.
              </p>
            </div>
            <LibraryHealth />
          </div>
        {:else if app.settingsTab === "playlist"}
          <div
            class="settings-section settings-section--tuning"
            onfocusin={rememberField}
          >
            <h4>Playlist</h4>
            <p class="settings-section-desc">
              What the auto-playlist queues, how often it tops up, and how long
              it keeps a track or an artist off the air. Out-of-range values are
              clamped on save.
            </p>

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

            <h5 class="tuning-group-title">Rotation</h5>
            <div class="device-row">
              <label for="tune-title-window">No-repeat title (minutes)</label>
              <input
                id="tune-title-window"
                type="number"
                min="0"
                max="10080"
                value={tuning.rotation.titleWindowMin}
                oninput={(e) =>
                  numInput(e, (v) => (tuning.rotation.titleWindowMin = v))}
                onchange={saveTuning}
              />
              <div class="hint">
                The auto-playlist will not pick a track that aired this
                recently. Music only. 0 turns it off.
              </div>
            </div>
            <div class="device-row">
              <label for="tune-artist-window">No-repeat artist (minutes)</label>
              <input
                id="tune-artist-window"
                type="number"
                min="0"
                max="10080"
                value={tuning.rotation.artistWindowMin}
                oninput={(e) =>
                  numInput(e, (v) => (tuning.rotation.artistWindowMin = v))}
                onchange={saveTuning}
              />
              <div class="hint">
                The auto-playlist will not pick a track whose artist aired this
                recently. Queued tracks count as aired. 0 turns it off.
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
              <label for="tune-history">History shown</label>
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
                How many aired tracks the History tab lists. Every airing is
                kept on record regardless. Minimum 1.
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
        {:else if app.settingsTab === "appearance"}
          <div class="settings-section">
            <h4>Theme</h4>
            <p class="settings-section-desc">
              A theme repaints the player. It changes no layout, no wording and
              no behaviour, and the app never repaints itself unasked — drop a
              folder in, then press <em>Reload themes</em>.
            </p>

            {#if app.appearance?.problem}
              <div class="theme-problem" role="status">
                <span class="material-symbols-outlined" aria-hidden="true"
                  >warning</span
                >
                {app.appearance.problem} — the colours on screen are the last good
                ones.
              </div>
            {/if}

            <div class="theme-list" role="radiogroup" aria-label="Theme">
              {#each app.themes as theme (theme.id + theme.source)}
                {@const active = app.appearance?.themeId === theme.id}
                <button
                  id={`appearance-theme-${theme.id}`}
                  class="theme-row"
                  class:active
                  class:invalid={theme.error !== null}
                  role="radio"
                  aria-checked={active}
                  disabled={theme.error !== null}
                  onclick={() => pickTheme(theme.id)}
                >
                  <span class="material-symbols-outlined theme-mark">
                    {theme.error !== null
                      ? "block"
                      : active
                        ? "radio_button_checked"
                        : "radio_button_unchecked"}
                  </span>
                  <span class="theme-text">
                    <span class="theme-name">{theme.name}</span>
                    <span class="theme-meta">
                      {theme.source}{theme.base
                        ? ` · ${theme.base}`
                        : ""}{theme.author ? ` · ${theme.author}` : ""}
                    </span>
                    {#if theme.error}
                      <span class="theme-error">{theme.error}</span>
                    {/if}
                  </span>
                </button>
              {/each}
            </div>

            <div class="theme-actions">
              <button id="appearance-reload" class="np-browse" onclick={reload}>
                <span class="material-symbols-outlined" aria-hidden="true"
                  >refresh</span
                >
                Reload themes
              </button>
              <button
                id="appearance-reveal"
                class="np-browse"
                onclick={() => void api.revealThemesDir()}
              >
                <span class="material-symbols-outlined" aria-hidden="true"
                  >folder_open</span
                >
                Show in folder
              </button>
            </div>
            <div class="hint">
              Themes live in <code>themes/</code> in the app data folder. Copy
              the <code>example</code> folder, edit the values, then reload.
            </div>
          </div>

          <div class="settings-section">
            <h4>Station Identity</h4>
            <p class="settings-section-desc">
              Your station's name and artwork. Kept separately from the theme,
              so they survive every theme you try — a theme may ship its own,
              and yours wins over it.
            </p>

            <div class="device-row">
              <label for="appearance-station-name">Station name</label>
              <input
                id="appearance-station-name"
                type="text"
                maxlength="64"
                placeholder={APP_NAME}
                bind:value={stationName}
                onchange={saveStationName}
              />
              <div class="hint">
                Shown in the toolbar and the window title. Leave it empty to use
                {APP_NAME}.
              </div>
            </div>

            {#each imageSlots as slot (slot.slot)}
              <div class="device-row">
                <label for={`appearance-${slot.slot}-choose`}
                  >{slot.label}</label
                >
                <div class="identity-row">
                  <span class="identity-preview" class:round={slot.round}>
                    {#if slot.src}
                      <img src={slot.src} alt="" />
                    {:else}
                      <span class="material-symbols-outlined" aria-hidden="true"
                        >image</span
                      >
                    {/if}
                  </span>
                  <button
                    id={`appearance-${slot.slot}-choose`}
                    class="np-browse"
                    onclick={() => pickImage(slot.slot)}>Choose…</button
                  >
                  <button
                    id={`appearance-${slot.slot}-clear`}
                    class="np-browse"
                    disabled={!slot.owned}
                    onclick={() => void app.clearStationImage(slot.slot)}
                    >Clear</button
                  >
                </div>
                <div class="hint">{slot.hint}</div>
              </div>
            {/each}
          </div>
        {:else if app.settingsTab === "advanced"}
          <div
            id="admin-mode-section"
            class="settings-section settings-section--tuning"
          >
            <h4>Admin Mode</h4>
            <p class="settings-section-desc">
              With a password set, settings, scanning, metadata edits and saving
              cue points to a track need it. This guards against mistakes; it is
              not a security boundary. Forgot it? Quit, delete
              <code>admin.passwordHash</code> from <code>config.json</code>, and
              relaunch.
            </p>
            {#if passwordFormOpen || !app.admin.passwordSet}
              <form onsubmit={savePassword}>
                <div class="device-row">
                  <label for="admin-new-password"
                    >{app.admin.passwordSet
                      ? "New password"
                      : "Password"}</label
                  >
                  <input
                    id="admin-new-password"
                    type="password"
                    autocomplete="new-password"
                    bind:value={newPassword}
                  />
                </div>
                <div class="device-row">
                  <label for="admin-confirm-password">Confirm password</label>
                  <input
                    id="admin-confirm-password"
                    type="password"
                    autocomplete="new-password"
                    bind:value={confirmPassword}
                  />
                </div>
                <div class="admin-actions">
                  <button
                    id="btn-admin-save-password"
                    type="submit"
                    class="btn-scan-now"
                    disabled={passwordBusy}
                    >{app.admin.passwordSet
                      ? "Change password"
                      : "Set password"}</button
                  >
                  {#if passwordFormOpen}
                    <button
                      type="button"
                      class="np-browse"
                      onclick={() => (passwordFormOpen = false)}>Cancel</button
                    >
                  {/if}
                </div>
              </form>
            {:else}
              <div class="admin-actions">
                {#if confirmingRemove}
                  <span
                    >Remove the password? Anyone can then change settings.</span
                  >
                  <button
                    id="btn-admin-remove-confirm"
                    class="btn-scan-now"
                    disabled={passwordBusy}
                    onclick={removePassword}>Remove</button
                  >
                  <button
                    class="np-browse"
                    onclick={() => (confirmingRemove = false)}>Keep</button
                  >
                {:else}
                  <button
                    id="btn-admin-change-password"
                    class="btn-scan-now"
                    onclick={() => (passwordFormOpen = true)}
                    >Change password</button
                  >
                  <button
                    id="btn-admin-remove-password"
                    class="np-browse"
                    onclick={() => (confirmingRemove = true)}
                    >Remove password</button
                  >
                {/if}
              </div>
            {/if}
            {#if passwordError}
              <div id="admin-password-error" class="admin-error" role="alert">
                {passwordError}
              </div>
            {/if}
            <div class="device-row">
              <label for="admin-idle-lock">Lock after idle (minutes)</label>
              <input
                id="admin-idle-lock"
                type="number"
                min="1"
                max="240"
                value={app.admin.idleLockMin}
                onchange={saveIdleLockMin}
              />
              <div class="hint">
                Admin mode locks again after this long without input, and on
                every launch.
              </div>
            </div>
          </div>

          <div
            class="settings-section settings-section--tuning"
            onfocusin={rememberField}
          >
            <h4>Advanced Tuning</h4>
            <p class="settings-section-desc">
              Fine-tune library checks, cue analysis, fades, buffering and
              network resilience. Out-of-range values are clamped on save.
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
            <div class="device-row">
              <label for="tune-tag-write-timeout"
                >Tag write timeout (seconds)</label
              >
              <input
                id="tune-tag-write-timeout"
                type="number"
                min="5"
                max="300"
                value={tuning.library.tagWriteTimeoutSec}
                oninput={(e) =>
                  numInput(e, (v) => (tuning.library.tagWriteTimeoutSec = v))}
                onchange={saveTuning}
              />
              <div class="hint">
                How long writing an edit into a file may take before it is
                reported as failed.
              </div>
            </div>

            <h5 class="tuning-group-title">Automatic cue analysis</h5>
            <div class="np-group" class:disabled={!tuning.autoCue.apply}>
              <div class="np-group-header">
                <span class="material-symbols-outlined" aria-hidden="true"
                  >content_cut</span
                >
                <span class="np-group-title">Apply automatic cue points</span>
                <label class="np-toggle" title="Apply automatic cue points">
                  <input
                    id="setting-apply-auto-cue"
                    type="checkbox"
                    bind:checked={tuning.autoCue.apply}
                    onchange={saveTuning}
                  />
                  <span class="np-toggle-track"></span>
                </label>
              </div>
              <p class="settings-section-desc">
                Trims and handover points derived from the audio are used on air
                and in every duration the app shows. Switched off, every track
                plays whole — but the analysis still runs and keeps its results,
                so switching back on takes effect immediately with no second
                pass over the library. Radio edits you made by hand always
                apply.
              </p>
              <div class="np-subsetting">
                <div class="np-group-header">
                  <span class="np-group-title">Apply automatic Next starts</span
                  >
                  <label class="np-toggle" title="Apply automatic Next starts">
                    <input
                      id="setting-apply-auto-next-start"
                      type="checkbox"
                      bind:checked={tuning.autoCue.applyNextStart}
                      disabled={!tuning.autoCue.apply}
                      onchange={saveTuning}
                    />
                    <span class="np-toggle-track"></span>
                  </label>
                </div>
                <p class="settings-section-desc">
                  Music hands over before it has finished, overlapping the
                  incoming item. Switched off, a track the analysis owns plays
                  to its Cue out and the next one starts clean — the derived
                  trims still apply. Next starts you set by hand always apply.
                </p>
              </div>
            </div>
            <div class="device-row">
              <label for="tune-silence-db">Silence threshold (dBFS)</label>
              <input
                id="tune-silence-db"
                type="number"
                min="-100"
                max="-4"
                step="1"
                value={tuning.autoCue.silenceDbfs}
                oninput={(e) =>
                  thresholdInput(e, (v) => (tuning.autoCue.silenceDbfs = v))}
                onchange={saveTuning}
              />
              <div class="hint">
                Below this there is no programme audio, so Cue in and Cue out
                trim it off each end of a track. Changing it affects later
                analyses only — nothing already analysed is recalculated.
              </div>
            </div>
            <div class="device-row">
              <label for="tune-segue-db">Segue threshold (dBFS)</label>
              <input
                id="tune-segue-db"
                type="number"
                min="-99"
                max="-3"
                step="1"
                value={tuning.autoCue.segueDbfs}
                oninput={(e) =>
                  thresholdInput(e, (v) => (tuning.autoCue.segueDbfs = v))}
                onchange={saveTuning}
              />
              <div class="hint">
                How quiet a music track has to get before the next item may
                start. Always kept above the silence threshold. Music only —
                commercials and jingles get no automatic Next start. Still
                derived and stored while automatic Next starts are switched off,
                so turning them back on costs no second pass.
              </div>
            </div>
            <div class="np-action-row">
              <button
                class="btn-scan-now"
                onclick={recalculateAutoCue}
                disabled={recalculating || scanning}
                title={scanning
                  ? "A library scan is running; recalculate when it finishes"
                  : "Apply these levels to tracks already analysed"}
                >{recalculating ? "Recalculating…" : "Recalculate now"}</button
              >
              {#if recalcResult}
                <span class="np-test-result">{recalcResult}</span>
              {/if}
            </div>
            <p class="settings-section-desc">
              New levels reach later analyses on their own. This applies them to
              everything already in the library — from each track's stored
              measurements where it has them, and by decoding again where it
              does not. Radio edits you made by hand are left alone.
            </p>

            <h5 class="tuning-group-title">Fades</h5>
            <div class="device-row">
              <label for="tune-fade-out">Fade out (ms)</label>
              <input
                id="tune-fade-out"
                type="number"
                min="200"
                max="30000"
                value={tuning.player.fadeOutMs}
                oninput={(e) =>
                  numInput(e, (v) => (tuning.player.fadeOutMs = v))}
                onchange={saveTuning}
              />
              <div class="hint">
                How long the deck's Fade out button takes to reach silence
                before stopping.
              </div>
            </div>
            <div class="device-row">
              <label for="tune-fade-next">Fade to next (ms)</label>
              <input
                id="tune-fade-next"
                type="number"
                min="200"
                max="30000"
                value={tuning.player.fadeToNextMs}
                oninput={(e) =>
                  numInput(e, (v) => (tuning.player.fadeToNextMs = v))}
                onchange={saveTuning}
              />
              <div class="hint">
                How long the outgoing track takes to fade under the incoming
                one. Usually shorter than a fade to silence.
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
