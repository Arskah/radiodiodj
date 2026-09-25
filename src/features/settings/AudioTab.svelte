<script lang="ts">
  import { app } from "../../shared/state.svelte";
  import type {
    DeviceInfo,
    DeviceRef,
    ReplayGainMode,
    TuningConfig,
  } from "../../shared/types";

  interface Props {
    /**
     * The overlay's tuning draft. Bindable because the fields here write into
     * it directly; the overlay is what persists it.
     */
    tuning: TuningConfig;
    saveTuning: (e?: Event) => Promise<void>;
  }

  let { tuning = $bindable(), saveTuning }: Props = $props();

  // A device swap only reaches the running decks on the next load, so the
  // notice stays up until the overlay is reopened.
  let mainDeviceChanged = $state(false);
  $effect(() => {
    if (app.settingsOpen) mainDeviceChanged = false;
  });

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
</script>

<div class="settings-section">
  <h4>Audio Configuration</h4>
  <p class="settings-section-desc">
    Configure your signal chain for low-latency broadcast performance.
  </p>
  {#if app.audioDevices.length === 0}
    <div class="empty">
      <span class="empty-icon"
        ><span class="material-symbols-outlined">volume_off</span></span
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
        <div class="hint">Restart required to apply main-device change.</div>
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
          tuning.player.replayGain = e.currentTarget.value as ReplayGainMode;
          saveTuning();
        }}
      >
        <option value="track">Level every track</option>
        <option value="off">Play as mastered</option>
      </select>
    </span>
    <div class="hint">
      Brings every track to the same loudness, so a quiet song does not
      disappear after a loud one and crossfades mix at the levels you hear.
      Applies to both outputs. Measured during the waveform pass — a track stays
      unlevelled until that reaches it.
    </div>
  </div>
</div>
