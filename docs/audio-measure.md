# Audio measurement — `audio_measure`

Everything this app learns about a file by decoding it: the waveform curve, the
integrated loudness, the level envelope behind the automatic cue points, the
tempo, and the content fingerprint. One module, one decode, no knowledge of
where the answers go.

**Planned.** This describes the module as it will stand; today the files are
still spread across `audio/` and `library/`. The move is the
[increments](#increments) at the end, none of which changes behaviour.

It is a library that happens to live in this repository. Nothing in it opens a
device, touches the database, reads a setting, emits a Tauri event or holds a
`Sink`. A caller hands it bytes and a threshold and receives numbers. That is
already how the code behaved before the move — `waveform.rs`, `auto_cue.rs`,
`loudness.rs` and `bpm.rs` were pure — but nothing said so, and nothing stopped
the next commit from reaching for `AppHandle` because the file next door had one.

## Why it is separate

The `audio/` folder held two unrelated jobs under one name.

One is **playback**: a `Sink` per deck, one `OutputStream` per device, a worker
ticking playheads at 50 ms, a watchdog over a network share, roles moving
between decks mid-air. It is concurrent, stateful, device-bound, and its
failure mode is dead air.

The other is **measurement**: decode a `Vec<u8>`, walk the samples once,
return structs. It is a pure function. Its failure mode is a wrong number in a
database row, found weeks later.

These want opposite things from a reviewer and from a test. Playback is tested
against a fake output and a clock; measurement is tested against a generated
WAV and an expected value. Playback changes when rodio changes; measurement
changes when the music does. Keeping them in one folder meant every
"where does this go" question had to be re-answered from scratch, and it meant
the pure half could not be lifted out to be used anywhere else.

The reuse is not hypothetical in shape, only in timing: a station has other
things that want a track's tempo, loudness and level envelope without wanting a
deck — a batch tool over an archive, a logger checking what aired, an importer
vetting new material. None of them should have to link a mixer to ask.

## What it holds

| file                           | holds                                                             |
| ------------------------------ | ----------------------------------------------------------------- |
| `audio_measure/mod.rs`         | the boundary, stated, and the guard test that keeps it            |
| `audio_measure/formats.rs`     | the supported-extension table — what this can be asked to read    |
| `audio_measure/waveform.rs`    | `analyze` — the one decode — plus `compute_detail` for the editor |
| `audio_measure/loudness.rs`    | `Loudness`, `TARGET_LUFS`, `gain_db`, `linear_gain`               |
| `audio_measure/auto_cue.rs`    | RMS windows, the level envelope, and the markers derived from it  |
| `audio_measure/bpm.rs`         | the onset envelope and the tempo estimator                        |
| `audio_measure/fingerprint.rs` | the tag-independent content hash of demuxed packets               |
| `audio_measure/test_audio.rs`  | `write_wav`, the generated fixture its own tests measure          |

Its outward surface is four entry points — `analyze`, `compute_detail`,
`fingerprint::of_source` / `of_file` — plus the types they return and the
derivations over those types (`RmsWindows::envelope`, `Envelope::detect`,
`Envelope::encode` / `decode`, `loudness::gain_db`).

## What stayed behind, and why

**`audio/cue_points.rs`** — the five markers. They are a _domain model_ shared by
the database, the playlist, the session and the decks, not a measurement.
`auto_cue` deliberately emits its own `AutoCue` type and never mentions
`CuePoints`, so the module has no need of it, and pulling it in would drag the
whole cue vocabulary across a boundary drawn to keep vocabulary out.

**`ReplayGainMode` and `loudness::factor`** — `factor(mode, gain_db, peak)` is
the one place the old `loudness.rs` reached into `persist::config`. What gain a
track _earns_ is arithmetic and stays; whether the operator wants levelling at
all is a setting, and a measurement library should not carry one. `factor` moves
to the app side beside its two callers (`lib.rs`, `playlist/service.rs`), which
is also where the answer to "no album mode" belongs.

**`library/waveform_scan.rs`** — the **Analysis pass**. It owns the `Db`, the
`AppHandle`, the queue, the progress events and the cancel token. The pass
orchestrates; the module measures. This is the load-bearing half of the split:
the module can be called in a loop, in one shot, from a test or from another
program, because nothing in it knows a queue exists.

**`audio/player.rs`'s read and decode helpers** — superficially the same work,
actually the opposite constraints. The playback decoder is built to be seeked
into two stages deep, wrapped in a gain envelope and handed to a `Sink`, under a
watchdog because the file is on a share that may be wedged. The measurement
decoder is built once over bytes already in RAM and walked to the end. Sharing
them would couple a pure function to a retry policy.

**`write_tag` and `retag_externally`** — the other half of the old
`library/test_audio.rs`. They write lofty tags, which is metadata, not audio.
The library's tag tests keep them; only `write_wav` moves.

## The rules

Inside `audio_measure`, none of the following may appear:

| banned                             | because                                             |
| ---------------------------------- | --------------------------------------------------- |
| `tauri::` anything                 | no events, no `AppHandle`, no command handlers      |
| `crate::library::`                 | it does not know a database exists                  |
| `crate::persist::`                 | a setting arrives as an argument or not at all      |
| `crate::playlist::`, `broadcast::` | it does not know what a station is                  |
| `rodio::Sink`, `cpal::`            | it never plays anything or opens a device           |
| `std::thread`, a channel           | the caller decides the concurrency; the pass has it |

`rodio::Decoder`, `symphonia`, `ebur128`, `anyhow` and `serde` are the
dependencies it is allowed, and `std::fs` only in `fingerprint::of_file`.

A `mod.rs` test walks the module's own `.rs` files and fails on any of the
banned strings. This is the same shape as the schema snapshot test and the
theming token-contract guard: the invariant is stated once, in code, where a
reviewer cannot miss it and a later commit cannot quietly cross it. A comment
would only be read by someone already looking.

## One decode, many measurements

`analyze` decodes a track once and runs four consumers off the single
`inspect()` walk over its samples:

| consumer               | keeps                                      | width                       |
| ---------------------- | ------------------------------------------ | --------------------------- |
| waveform mip buffer    | summed squares per cell → 400 `u8` buckets | bounded, merges on overflow |
| `EbuR128` meter + peak | integrated loudness, highest sample        | 8192-sample chunks          |
| `auto_cue::Collector`  | RMS per window → the level envelope        | 50 ms                       |
| `bpm::Collector`       | RMS per frame → the onset envelope         | 10 ms, capped at 5 min      |

The decode dominates the cost of any one of them, which is why they share it,
and it is the reason the fingerprint pass reuses the same bytes rather than
reading the file again. A fifth consumer is the cheap way to ask a new question
of a library — the reason key detection is drawn as another collector rather
than another pass.

Everything kept here is orders of magnitude smaller than the audio it describes:
the level envelope is one byte per 50 ms window, the onset envelope 100 floats
per second against ~1.4 MB/s of samples. That is what makes eight concurrent
workers affordable.

## The word

CONTEXT.md defines **Analysis pass** as the background decode that follows a
scan — the queue walker in `library/waveform_scan.rs`. The module is not that
pass, which is why it is not called `analysis`. The pass is _when and for which
rows_; the module is _what a decode says_. A new CONTEXT.md entry names the
second thing so the two cannot merge in conversation:

> **Measurement**: what one decode of a Track's audio yields — waveform curve,
> loudness, **Level envelope**, tempo, fingerprint. Produced by `audio_measure`,
> requested and stored by the **Analysis pass**. _Avoid_: analysis (that is the
> pass), stats, metrics.

## Becoming a crate

Not now. The module standing alone inside the binary is what earns the move, and
carrying it as a crate before anything else links it would buy a `Cargo.toml`
and a version number in exchange for nothing. When a second consumer exists,
extraction is a directory move plus three loose ends:

- **Fixtures.** `write_wav` is `#[cfg(test)]`, so library tests reaching for it
  across a crate boundary would need it behind a `test-fixtures` feature, or
  their own copy.
- **`anyhow` in a public signature.** Fine in a binary, rude in a library —
  `analyze` and `of_source` would want typed errors before anyone else depends
  on them.
- **Threshold arguments.** `analyze(bytes, silence_dbfs)` and
  `Envelope::detect(music, thresholds)` take the app's numbers as arguments,
  which is correct, but `Thresholds` is currently built in `persist::config`. A
  crate would want its own `Default`.

The Cargo name would be `audio-measure`; Cargo normalises the hyphen, so the
module path does not change when that day comes.

## Increments

Each lands on its own, and none changes behaviour.

1. **The module exists.** Move `waveform.rs`, `auto_cue.rs`, `loudness.rs`,
   `bpm.rs` and `formats.rs` to `audio_measure/`. Move `factor` and
   `ReplayGainMode`'s use to the app side. Re-import across ~12 files, update
   `docs/audio.md`'s code map, `docs/architecture.md`'s module list and
   AGENTS.md, and drop this doc's **Planned** marker in `docs/README.md`.
   `analyze`'s doc comment still says "three outputs" and names three consumers;
   tempo made it four.
2. **The fingerprint joins it.** Move `library/fingerprint.rs`, and `write_wav`
   out of `library/test_audio.rs`. Touches `db.rs`, `scanner.rs`,
   `tag_write.rs`, `waveform_scan.rs`, `check.rs` and
   `docs/track-identity.md`.
3. **The boundary gets teeth.** The guard test in `mod.rs` and the CONTEXT.md
   **Measurement** entry.

Ordered so the guard lands over a module that is already complete — a guard
written first would only have to be edited twice.
