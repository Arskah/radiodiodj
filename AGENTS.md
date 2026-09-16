# AGENTS.md

## Build & Run Commands

```bash
pnpm dev                                          # tauri dev (Vite HMR for renderer, cargo watch for backend)
pnpm build                                        # tauri build → src-tauri/target/release/bundle/<format>/
pnpm typecheck                                    # svelte-check + tsc on tsconfig.node.json
pnpm test                                         # vitest watch
pnpm test -- run                                  # vitest single run (159 renderer tests)
pnpm e2e                                          # tauri-driver + WebdriverIO (Linux only — see e2e/README.md)
cargo test --manifest-path src-tauri/Cargo.toml   # 211 backend tests (db, scanner, session, playlist, audio, config)
pnpm lint                                         # eslint
pnpm format                                       # prettier --write .
pnpm format:check                                 # prettier --check .
```

## Architecture

Tauri 2 app. Two process boundaries: a Rust backend and a Svelte 5 / Vite renderer talking over Tauri `invoke` + `emit`/`listen`.

**Rust backend** (`src-tauri/src/`) — grouped by domain:

- `lib.rs` — `tauri::Builder` setup (`tauri-plugin-log` first, then `tauri-plugin-dialog`), `AppState`, all `#[tauri::command]` handlers, panic hook (force-capture backtrace → `log::error!`)
- `main.rs` — thin `pub fn main() { radiodiodj_lib::run() }` binary entry
- `audio/` — `formats.rs` (supported extension table), `player.rs` (shared deck primitives: the `Cmd` vocabulary, the `Topics` table, whole-file read/retry and symphonia decode for mp3/flac/vorbis/wav/aac/m4a), `output.rs` (one `OutputStream` per device, opened lazily with self-healing retry), `deck.rs` (one rodio `Sink` per deck plus the worker loop that ticks a whole set of them against one output; emits `{role}:time` (10 Hz) / `:duration` / `:pause-state` / `:ended` / `:error` / `:buffering` / `:load-failed` / `:output-unavailable`), `bus.rs` (the program bus — deck A + deck B on one mixer, one worker, `program:roles`), `cue.rs` (the cue deck: one deck on its own output, `cue:*`), `cue_points.rs` (the five per-track markers and their resolution to file positions — pure, no device), `envelope.rs` (the `Enveloped<I>` source applying a track's stored fades)
- `library/` — `db.rs` (rusqlite + FTS5, WAL, `parking_lot::Mutex<Connection>`, `user_version` migrations), `scanner.rs` + `scan_state.rs` (recursive walkdir scan, `lofty` tag extraction, mtime+content_type delta cache, background worker thread emitting `scan-progress` / `scan-state-changed` events with cancel token)
- `persist/` — `config.rs` (`AppConfig` → `{app_data_dir}/config.json`), `session.rs` (`SessionState` per-field defaults → `{app_data_dir}/session.json`)
- `playlist/` — owner of the playlist and of everything that advances it: `generate.rs` (random selection with jingle/commercial interleaving, every-4 / every-8), `engine.rs` (the pure state machine — queueing, advancement, outage skip-to-cached, refill, stop markers — returning effects), `model.rs` (wire types incl. the `program:playlist-state` snapshot), `service.rs` (effects → deck commands, play counts, prefetch window, retry timers)

**Frontend** (`src/`) — Vite root + Tauri Svelte template convention:

- `main.ts` — app entry; mounts Svelte, hooks Tauri `onCloseRequested` to await `flushSave()` before `win.destroy()`
- `App.svelte` — top-level UI tree
- `shared/` — `types.ts`, `api.ts` (typed `invoke()` wrapper, folder picker via `@tauri-apps/plugin-dialog`), `state.svelte.ts` (Svelte 5 `$state` store; deck transport via `DeckTransport`, playlist state mirrored from backend snapshots), colocated `state.test.ts` + `mockBackend.ts` + `mockPlaylist.ts`
- `features/<feature>/` — one folder per UI feature: `library/`, `playlist/`, `deck/` (NowPlaying.svelte + CueDeck.svelte + Waveform.svelte + backend.ts + nativeBackend.ts), `scan/`, `settings/` (SettingsOverlay.svelte — Library + Audio tabs), `toolbar/`, `track/` (TrackTooltip.svelte + MetadataOverlay.svelte + CuePointOverlay.svelte)

## Key Patterns

**Audio playback:** in-process Rust decks. The playlist loads tracks onto the main deck; the worker thread decodes via symphonia and emits time/duration/pause-state/ended/error events. No browser `<audio>` element, no `media://` protocol, no transcoder.

**Program bus:** every on-air deck is a `Sink` on one shared `OutputStream` mixer, driven by a single worker thread, so two decks can be audible at once (the precondition for handover). Deck events are **role-mapped**: whichever deck holds `main` emits `main-deck:*`, the armed one emits `arm-deck:*` — the renderer, broadcast service and now-playing webhook never learn which physical deck is on air. Roles are static today (slot A is `main`); handover is a later increment. The cue deck is deliberately off the bus with its own stream, thread and `cue:*` topics. See `docs/program-bus.md`.

**Cue points:** five nullable per-track markers (`cue_in_ms`, `fade_in_ms`, `fade_out_ms`, `cue_out_ms`, `next_start_ms`) stored as columns on `tracks`, deliberately absent from `UPSERT_TRACK_SQL` so a rescan cannot destroy them. `Cmd::Load` carries concrete `CuePoints`; the worker resolves them against the _decoded_ duration (the tag one is wrong on VBR MP3) and plays `take_duration(cueOut − pos)`, so the existing `sink.empty()` → `:ended` path ends a trimmed track with no new termination rule. Everything crossing the Tauri boundary is **air time**, measured from `cue_in` — a trimmed track is simply a shorter track to the renderer. Stored fades are a **source-level** envelope (`audio/envelope.rs`), not a `sink.set_volume()` ramp: a live fade and a stored fade would otherwise fight over one value, whereas a source envelope times a sink gain composes by multiplication. A track with no ramps is handed to the sink unwrapped. Clamping is backend-owned: `set_cue_points` returns what it stored, and `shared/cuePoints.ts` only ever applies the `null` fallbacks — there is no TypeScript clamp. Item overrides are a later increment. See `docs/cue-points.md`.

**Durations mean air time.** Library rows, playlist rows, the history list, both decks and the now-playing webhook all report `cueOut − cueIn`, via `airDuration()`; `TrackTooltip` carries the file length too when the two differ. The renderer never optimistically shows file time, because the deck reports air time and the two would disagree mid-load.

**Authoring cue points.** `CuePointOverlay.svelte` (library row → _Cue points…_, or the cue deck's marker button) is the editor: `Waveform.svelte` with draggable handles plus a millisecond field per marker. The cue deck auditions in two modes that never mix — _Absolute_ plays the whole file so an in-point can be scrubbed for, _Preview_ reloads with markers applied and crops the waveform to the aired region. Switching reloads the deck because markers are applied at load time. `cue_load` takes an optional `cuePoints`, so the editor can preview an unsaved draft. Saving a radio edit deliberately does **not** touch `currentTrack`: it applies from the next airing, so on-air audio never re-decodes under the operator.

**Naming:** "edit" means metadata and nothing else (`MetadataOverlay.svelte`, `app.editingMetadata`); playback markers are always "cue points" (`CuePointOverlay.svelte`, `app.editingCuePoints`).

**Search:** FTS5 virtual table on title/artist/album/genre. Triggers keep FTS in sync with tracks table. Query tokenized as prefix match: `foo bar` → `"foo"* "bar"*`.

**Scan + prune:** On scan, the scan worker walks the configured paths, deletes DB rows whose path no longer falls under any configured library path, then upserts present files. Empty paths array → all tracks deleted.

**Playlist ownership:** the backend owns the playlist, what is on air, and advancement. The renderer sends `playlist_*` commands and mirrors the `program:playlist-state` snapshot that comes back — it keeps no playlist of its own. History is the one exception: a renderer-side display log fed by each snapshot's `displaced` track. See `docs/backend-owned-playlist.md`.

**Auto-playlist:** Toggle mode that keeps a lookahead buffer queued, refilling from random DB selection when it drops below the threshold. Runs in `playlist::engine` alongside advancement, so a refill and the track change that triggered it are one transition.

**Seek:** `audio/player.rs` reloads the source on seek. `append_span` seeks in two stages — `try_seek` (container-level binary search) to ~200 ms short of the target, then `skip_duration` (sample iteration) for the remainder — so a stored marker lands sample-exactly even where symphonia estimates the seek by bitrate. A failing `try_seek` falls back to `skip_duration` from zero. `seek_offset + sink.get_pos()`, less `cue_in`, keeps `{role}:time` accurate.

## Gotchas

- `tauri::generate_context!()` runs at compile time and validates `frontendDist=../dist`. `cargo clippy` / `cargo test` panic with "frontendDist path doesn't exist" unless `pnpm vite build` has run; CI does this in `rust.yml` before cargo steps.
- `serde(default)` per-field on `SessionState` / `AppConfig` lets new fields land without a schema version bump. Match this pattern when adding fields.
- pnpm `minimumReleaseAge` constraint blocks plugin versions younger than ~3 days; pin to a slightly older stable version when adding `tauri-plugin-*` deps.
- Tauri command argument name `state` collides with the `State<AppState>` injection; the managed state arg is named `app` in command handlers.
- `release-please-config.json` bumps `package.json`, `src-tauri/tauri.conf.json` (jsonpath `$.version`), and `src-tauri/Cargo.toml` (`# x-release-please-version` annotation) on each release. Keep all three in sync.
- `tauri-plugin-log` is initialized first in the builder chain so panics before later plugin setup still reach the file sink. Renderer `console.*` is intercepted by `attachConsole()` in `main.ts`; vitest must not import `main.ts` (it doesn't — tests use `mockBackend`). Log level honors `RUST_LOG` (whole-app level only — no module syntax) and falls back to `Debug` in `cfg!(debug_assertions)` / `Info` in release. `symphonia*` modules are forced to `Warn` to keep the webview console readable.

## Data files

RadiodioDJ stores its database (`radiodiodj.db`), config (`config.json`), session
state (`session.json`), and default now-playing output in a per-user data
directory.

- macOS: `~/Library/Application Support/com.radiodiodj/`
- Linux: `~/.local/share/com.radiodiodj/` (or `$XDG_DATA_HOME/com.radiodiodj/`)
- Windows: `%APPDATA%\com.radiodiodj\` (typically `C:\Users\<you>\AppData\Roaming\com.radiodiodj\`)

## Logs

RadiodioDJ writes a rotating log file (`RadiodioDJ.log`, 1 MB max, one prior file kept).

- macOS: `~/Library/Logs/com.radiodiodj/RadiodioDJ.log`
- Linux: `~/.local/share/com.radiodiodj/logs/RadiodioDJ.log` (or `$XDG_DATA_HOME/com.radiodiodj/logs/`)
- Windows: `%LOCALAPPDATA%\com.radiodiodj\logs\RadiodioDJ.log`

Set `RUST_LOG=debug` (or `trace`) before launching to raise verbosity. Default is `info` (release) or `debug` (dev).

## Testing

Pre-commit hook runs lint-staged (prettier + eslint fix) then vitest.

CI gates Rust with `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` (`.github/workflows/rust.yml`); run both locally before pushing.
