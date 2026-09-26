# Audio — decode, output devices and levels

The audio path from a file on disk to a sample on an output device: how a track
is read and decoded, which device it lands on, how loud it plays, and what
"duration" means once cue points apply.

The on-air topology — the mixer, the deck roles, handover and the live fade
buttons — is [program-bus.md](./program-bus.md). The markers this module
consumes are [cue-points.md](./cue-points.md). This page covers everything
underneath both.

| topic                              | detail                                         |
| ---------------------------------- | ---------------------------------------------- |
| mixer, roles, handover, live fades | [program-bus.md](./program-bus.md)             |
| the five markers and their editor  | [cue-points.md](./cue-points.md)               |
| deriving markers from a decode     | [cue-auto-analysis.md](./cue-auto-analysis.md) |
| where the modules sit              | [architecture.md](./architecture.md)           |

## The decode path

Playback is in-process Rust: [rodio](https://docs.rs/rodio) driving
[symphonia](https://docs.rs/symphonia), on worker threads inside the Tauri
backend. There is no browser `<audio>` element, no `media://` protocol, no
transcoder and no external binary to bundle or sign — the Chromium audio
pipeline was replaced wholesale in the Tauri port (#76), and the earlier plan to
shell out to mpv was dropped with it.

Supported extensions are the table in `audio_measure/formats.rs`:

```
mp3  flac  wav  ogg  oga  aac  m4a  opus  webm  aiff  aif  mka  mp2
```

A scan indexes these and skips everything else. WMA is deliberately absent —
see [library.md](./library.md#unsupported-formats) for converting a folder of
it.

`audio/player.rs` holds what every deck shares: the `Cmd` vocabulary a deck
worker accepts, the `Topics` table naming its events, and the read/decode
helpers. `audio/deck.rs` is one deck — a rodio `Sink` plus its load, seek and
end-of-track bookkeeping — and the worker loop that ticks a whole set of decks
against one output.

### Whole-file reads

A deck does not stream from the filesystem. It reads the **whole file into RAM**
and decodes from a `Cursor` over those bytes. On a local disk this is merely
tidy; on the SMB or NFS share a station actually keeps its music on it is the
difference between a stall and a broadcast, because a share that wedges mid-file
cannot stall a decoder that is no longer reading from it.

The read happens on its own thread, with two protections in `audio/player.rs`:

- **Retry.** Four attempts with backoff between them, covering transient errors.
- **A watchdog.** `READ_WATCHDOG_TIMEOUT` (10 s) bounds how long the worker
  waits. A `read()` blocked on a dead mount cannot be cancelled, so the thread
  is abandoned rather than joined; it unwinds whenever the OS finally errors
  the mount.

A read that fails or times out emits `{role}:load-failed`, which the playlist
engine turns into skip-to-cached and a retry timer — see
[playlist.md](./playlist.md#outages).

### The prefetch cache

`audio/cache.rs` keeps whole track files resident in RAM, keyed by track id, so
a deck load finds its bytes already fetched. Residency is a **whole-playlist
window**, not an LRU: the playlist service pushes the upcoming ids (current
first, then playlist order), entries outside the latest window are evicted at
once, and the window is walked front-first until `MAX_CACHE_BYTES` is reached.
So the whole playlist is attempted and the nearest tracks win.

One background worker fetches missing entries sequentially, never in parallel —
hammering a network share with concurrent reads is how a share that was merely
slow becomes a share that is down.

## Output devices

`audio/output.rs` owns one `OutputStream` per physical device. Every deck driven
by the same worker shares that one output, which is precisely what makes the
program bus a mixer instead of a set of independent streams.

The stream is opened **lazily and re-openably**: a failure at launch never
permanently disables playback (#259), and a device that comes back is picked up
by the next open. A fixed 4096-frame buffer is requested for predictable
latency, falling back to the device default when a host (CoreAudio, commonly)
rejects the request rather than killing the worker thread.

`audio/devices.rs` enumerates outputs through `cpal` and reports
`{ name, description, isDefault }`. The operator picks devices under _Settings →
Audio_; the choice is stored in `config.json` as a `DeviceRef { name,
description }` rather than an index or an opaque handle, so it survives the
device list changing shape. On boot the stored `name` is matched exactly, then
the `description` is matched as a fallback with a warning, and failing both the
main output falls back to the system default. The cue device is optional: with
none configured, the cue deck is simply not spawned.

### Exclusive output

Not implemented, and deliberately so. Exclusive mode means WASAPI's
`AUDCLNT_SHAREMODE_EXCLUSIVE` on Windows, `kAudioDevicePropertyHogMode` on
macOS, and opening `hw:N,M` directly on ALSA. cpal 0.16 — the version rodio
pulls — exposes none of the three: WASAPI is hardcoded to shared mode,
CoreAudio has no reference to hog mode in the crate, and ALSA opens whatever
name enumeration returned.

Shipping a toggle that does nothing would mislead an operator into believing
they have a device they do not, so there is no toggle. Three ways out exist if
it is ever prioritised — wait for upstream cpal (RustAudio/cpal#598, #743),
write a ~300 LOC per-platform adapter below cpal, or vendor a patched cpal —
and option one is the recommendation. A station needing bit-exact output today
can dedicate a USB interface to the app and accept shared-mode mixing.

Worth knowing regardless of the path: while exclusive mode is engaged, every
other application on that device goes silent (Slack calls, browser audio, system
alerts); exclusive mode requires the device's raw mix format, so a sample-rate
mismatch would need a resampling step the OS mixer currently provides for free;
and a hot-unplug that shared mode recovers from by rerouting would instead
require an explicit teardown and reopen.

## The cue deck

`audio/cue.rs`. One off-air deck, on its own output device, with its own output
stream, its own worker thread and its own `cue:*` topics. It is **not** on the
program bus — it is monitoring on a different physical device, not program
audio, and mixing it into the bus would put headphone content on air. It reuses
the deck worker for everything that is the same everywhere: whole-file reads,
the watchdog, the self-healing open.

It is the audition surface for the cue editor, in two modes that never mix:

- **Absolute** — plays the whole file, so an in-point can be scrubbed for.
- **Preview** — reloads with the draft markers applied and crops the waveform to
  the aired region.

Switching modes reloads the deck, because markers are applied at load time.
`cue_load` takes an optional `cuePoints` (so an unsaved draft can be auditioned)
and an `autoplay` flag; the deck parks its sink when the background read lands,
so a `Play` sent alongside a `Load` would be undone by it. Cueing and mode
switching stay parked, and the editor's _Play_, _Audition_ and pre-roll are the
only explicit asks. See [cue-points.md](./cue-points.md#authoring).

The cue deck writes no history and increments no play count. Only what reaches
the `main` role is an airing.

## ReplayGain

Every track is levelled to one reference so a sparse 1970s master and a modern
loudness-war master do not step on each other on air.

`rg_gain`, `rg_peak` and `rg_measured_at` live on `tracks`, **measured by the
analysis pass** (`audio_measure/waveform.rs::analyze`, one decode that also
yields the RMS curve, the automatic-cue windows and the tempo) rather than read
from
`replaygain_track_gain` tags. Two reasons:

- Most station libraries are largely untagged. Normalising only the tagged half
  would pull those tracks toward reference while the rest stayed at full scale —
  a level split where there was none.
- Tags are not comparable with each other anyway. ReplayGain 1.0 (89 dB
  reference) and 2.0 (-18 LUFS, EBU R128) write the same tag name from different
  targets.

`rg_measured_at` is what "measured" means, because a silent or very short file
legitimately has no gain: a `NULL` gain with a timestamp is a measurement, a
`NULL` gain without one is work not done yet.

`audio_measure/loudness.rs` owns the arithmetic — `TARGET_LUFS = -18.0`, the gain that
brings a measurement to it, and the linear factor that gain earns once clamped
so the loudest sample lands no higher than full scale. A track already peaking
near 1.0 cannot be turned up; that is ordinary ReplayGain behaviour.

Two invariants:

- **`Cmd::Load` carries an already-resolved linear factor**, exactly as it
  carries concrete `CuePoints`. The deck worker never consults the library.
- **The factor is applied at the source**, in `append_span` — not through
  `sink.set_volume()`, for the same reason the fade envelope is not (below) —
  and therefore before the bus mixer sums the decks, so a handover between
  tracks mastered at different levels crossfades correctly.

The same function serves the cue deck, whose headphone output never passes the
station's processing chain.

The setting is `player.replayGain` (`off` / `track`), and
`audio/levelling.rs::factor` is the one place it meets a measurement — the split
that keeps `audio_measure/` free of settings. There is deliberately no
album mode: a radio playlist is a sequence of singles, and album gain would
reintroduce exactly the between-track level differences track gain exists to
remove. See #80.

## The fade envelope

A track's stored fade-in and fade-out are a **source-level** envelope,
`Enveloped<I>` in `audio/envelope.rs`, not a `sink.set_volume()` ramp.

Stored fades are "at position X through position Y" operations, keyed to
absolute file position. The live fade buttons are "from now, over N ms"
operations on the deck. Routing both through one value makes them fight — a
live fade fired while a track is inside its own stored fade-out clobbers it,
last writer per tick wins. As a source envelope multiplied by a sink gain they
compose at different stages, with no priority rule to get wrong. The envelope is
also sample-accurate rather than stepped at the worker's 50 ms tick.

`gain_at(pos, cue)` has five branches: before the in-point, the ramp up, the
body, the ramp down, past the out-point. A ramp whose two positions coincide has
zero width and contributes nothing, and a track with no ramps at all is handed
to the sink unwrapped.

(rodio 0.21 cannot express an outro ramp on its own: `fade_out` and
`linear_gain_ramp` both ramp from the source's start, and
`TakeDuration::set_filter_fadeout` fades across the entire take.)

## Durations mean air time

Every duration that crosses the Tauri boundary is **air time** — `cueOut −
cueIn`, via `airDuration()` — not file length. A trimmed track is simply a
shorter track to the renderer.

Library rows, playlist rows, the history list, both decks and the now-playing
webhook all report air time. So do the Upcoming tab total and the main deck's
remaining-on-air countdown, both of which stop at the first stop marker. The
toolbar's library _Playtime_ figure alone stays file time, because it answers
"how much audio do I have", not "how long will this play".

`TrackTooltip` carries the file length too when the two differ. The renderer
never optimistically shows file time while a load is in flight, because the deck
reports air time and the two would visibly disagree mid-load.

Position works the same way: `0` on the air timeline is the first audible
sample. Only the player worker knows source-absolute positions.

## Seek

`audio/player.rs` reloads the source on seek — there is no seek on a live
`Sink`.

`append_span` then seeks in two stages:

1. `try_seek` — the container-level binary search — to roughly 200 ms short of
   the target.
2. `skip_duration` — sample iteration — for the remainder.

The second stage is what makes a stored marker land sample-exactly even in
formats where symphonia estimates the seek position from the bitrate. A
`try_seek` that fails outright falls back to `skip_duration` from zero.

`seek_offset + sink.get_pos()`, less `cue_in`, is what keeps `{role}:time`
accurate afterwards.

## Events

Deck events are role-mapped: whichever deck holds `main` emits `main-deck:*`,
the armed one `arm-deck:*`, an outgoing one `tail-deck:*`. The cue deck emits
`cue:*`. Per deck:

| topic                       | meaning                                         |
| --------------------------- | ----------------------------------------------- |
| `{role}:time`               | air-timeline position, 10 Hz                    |
| `{role}:duration`           | air time of the loaded track                    |
| `{role}:pause-state`        | paused / playing                                |
| `{role}:ended`              | the track reached its cue out or the file's end |
| `{role}:loaded`             | bytes are in the sink                           |
| `{role}:load-failed`        | the read or decode failed                       |
| `{role}:buffering`          | waiting on a read                               |
| `{role}:error`              | anything else the worker could not recover from |
| `{role}:output-unavailable` | the output device could not be opened           |

`program:roles`, `program:handover` and `program:faded-out` belong to the bus —
see [program-bus.md](./program-bus.md#events-and-commands).

## Code map

Playback. What a decode _measures_ — the waveform curve, the loudness numbers,
the level envelope, the tempo and the extension table — is `audio_measure/`; see
[audio-measure.md](./audio-measure.md).

| file                  | holds                                                       |
| --------------------- | ----------------------------------------------------------- |
| `audio/player.rs`     | `Cmd`, `Topics`, whole-file read + retry + watchdog, decode |
| `audio/cache.rs`      | the prefetch window and its fetch worker                    |
| `audio/output.rs`     | one `OutputStream` per device, self-healing open            |
| `audio/devices.rs`    | cpal enumeration, `DeviceRef` resolution                    |
| `audio/deck.rs`       | one `Sink` per deck plus the worker loop over a deck set    |
| `audio/bus.rs`        | the program bus                                             |
| `audio/cue.rs`        | the cue deck                                                |
| `audio/cue_points.rs` | the five markers, resolved against a decoded duration       |
| `audio/envelope.rs`   | `Enveloped<I>`, `gain_at`                                   |
| `audio/levelling.rs`  | `factor` — the ReplayGain setting over a measurement        |
