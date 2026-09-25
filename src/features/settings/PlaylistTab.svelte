<script lang="ts">
  import type { TuningConfig } from "../../shared/types";
  import { listInput, numInput, rememberField } from "./numericInput";

  interface Props {
    /**
     * The overlay's tuning draft. Bindable because the fields here write into
     * it directly; the overlay is what persists it.
     */
    tuning: TuningConfig;
    saveTuning: (e?: Event) => Promise<void>;
  }

  let { tuning = $bindable(), saveTuning }: Props = $props();
</script>

<div
  class="settings-section settings-section--tuning"
  onfocusin={rememberField}
>
  <h4>Playlist</h4>
  <p class="settings-section-desc">
    What the auto-playlist queues, how often it tops up, and how long it keeps a
    track or an artist off the air. Out-of-range values are clamped on save.
  </p>
  <h5 class="tuning-group-title">Interleave</h5>
  <div class="device-row">
    <label for="tune-jingle-every">Jingle every N tracks</label>
    <input
      id="tune-jingle-every"
      type="number"
      min="0"
      value={tuning.interleave.jingleEvery}
      oninput={(e) => numInput(e, (v) => (tuning.interleave.jingleEvery = v))}
      onchange={saveTuning}
    />
    <div class="hint">
      One jingle after this many music tracks. 0 disables jingles.
    </div>
  </div>
  <div class="device-row">
    <label for="tune-commercial-every">Commercial every N tracks</label>
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
      One commercial break after this many music tracks. 0 disables commercials.
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
        numInput(e, (v) => (tuning.interleave.commercialBucketMultiplier = v))}
      onchange={saveTuning}
    />
    <div class="hint">
      Commercials per break scale with playlist length times this factor. Higher
      = more ads per break.
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
        numInput(e, (v) => (tuning.interleave.commercialBucketMin = v))}
      onchange={saveTuning}
    />
    <div class="hint">
      Floor on commercials per break, regardless of playlist length. 0 disables
      the floor.
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
      oninput={(e) => numInput(e, (v) => (tuning.rotation.titleWindowMin = v))}
      onchange={saveTuning}
    />
    <div class="hint">
      The auto-playlist will not pick a track that aired this recently. Music
      only. 0 turns it off.
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
      oninput={(e) => numInput(e, (v) => (tuning.rotation.artistWindowMin = v))}
      onchange={saveTuning}
    />
    <div class="hint">
      The auto-playlist will not pick a track whose artist aired this recently.
      Queued tracks count as aired. 0 turns it off.
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
        numInput(e, (v) => (tuning.autoPlaylist.autoPlaylistBuffer = v))}
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
        numInput(e, (v) => (tuning.autoPlaylist.autoPlaylistThreshold = v))}
      onchange={saveTuning}
    />
    <div class="hint">
      Refill kicks in when the queue drops to this many tracks. Clamped to at
      most the buffer size.
    </div>
  </div>
  <div class="device-row">
    <label for="tune-history">History shown</label>
    <input
      id="tune-history"
      type="number"
      min="1"
      value={tuning.autoPlaylist.historyCap}
      oninput={(e) => numInput(e, (v) => (tuning.autoPlaylist.historyCap = v))}
      onchange={saveTuning}
    />
    <div class="hint">
      How many aired tracks the History tab lists. Every airing is kept on
      record regardless. Minimum 1.
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
        numInput(e, (v) => (tuning.autoPlaylist.sessionSaveThrottleMs = v))}
      onchange={saveTuning}
    />
    <div class="hint">
      Minimum gap between session writes to disk. 0 saves on every change (more
      disk I/O).
    </div>
  </div>
  <div class="device-row">
    <label for="tune-net-backoffs">Network retry backoffs (ms)</label>
    <input
      id="tune-net-backoffs"
      type="text"
      value={tuning.autoPlaylist.netRetryBackoffsMs.join(", ")}
      oninput={(e) =>
        listInput(e, (v) => (tuning.autoPlaylist.netRetryBackoffsMs = v))}
      onchange={saveTuning}
    />
    <div class="hint">
      Comma-separated wait times between refill retries. The last value repeats
      until recovery.
    </div>
  </div>
</div>
