# AGENTS.md

## Build & Run Commands

```bash
pnpm dev                                          # tauri dev (Vite HMR for renderer, cargo watch for backend)
pnpm build                                        # tauri build → src-tauri/target/release/bundle/<format>/
pnpm typecheck                                    # svelte-check + tsc on tsconfig.node.json
pnpm test                                         # vitest watch
pnpm test -- run                                  # vitest single run
pnpm e2e                                          # tauri-driver + WebdriverIO (Linux only — see e2e/README.md)
cargo test --manifest-path src-tauri/Cargo.toml   # backend tests (db, scanner, session, playlist, audio, config)
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
- `library/` (overview: `docs/library.md`) — `db.rs` (rusqlite + FTS5, WAL, `parking_lot::Mutex<Connection>`, append-only `rusqlite_migration` steps with a `schema.sql` snapshot, pre-migration backup, newer-DB refusal), `scanner.rs` + `scan_state.rs` (recursive walkdir scan, `lofty` tag extraction, mtime+content_type delta cache, one reconcile transaction per scan, background worker thread emitting `scan-progress` / `scan-state-changed` events with cancel token), `listing.rs` (root listing plus the changed and gone rules, shared by scan and check), `fingerprint.rs` (tag-independent content hash of the first MiB of demuxed packets), `waveform_scan.rs` (background pass filling waveforms and fingerprints), `health.rs` (the `library-health` report: missing tracks, exact and possible duplicates, unreadable tracks, dismissals, failed tag writes), `check.rs` (the timed library check — listing and stat only, never writes), `tag_write.rs` (opt-in write-back of metadata edits into the files' tags)
- `persist/` — `config.rs` (`AppConfig` → `{app_data_dir}/config.json`), `session.rs` (`SessionState` per-field defaults → `{app_data_dir}/session.json`)
- `playlist/` — owner of the playlist and of everything that advances it: `generate.rs` (random selection with jingle/commercial interleaving, every-4 / every-8), `engine.rs` (the pure state machine — queueing, advancement, outage skip-to-cached, refill, stop markers — returning effects), `model.rs` (wire types incl. the `program:playlist-state` snapshot), `service.rs` (effects → deck commands, play counts, prefetch window, retry timers)

**Frontend** (`src/`) — Vite root + Tauri Svelte template convention:

- `main.ts` — app entry; mounts Svelte, hooks Tauri `onCloseRequested` to await `flushSave()` before `win.destroy()`
- `App.svelte` — top-level UI tree
- `shared/` — `types.ts`, `api.ts` (typed `invoke()` wrapper, folder picker via `@tauri-apps/plugin-dialog`), `state.svelte.ts` (Svelte 5 `$state` store; deck transport via `DeckTransport`, playlist state mirrored from backend snapshots), colocated `state.test.ts` + `mockBackend.ts` + `mockPlaylist.ts`
- `features/<feature>/` — one folder per UI feature: `library/`, `playlist/`, `deck/` (NowPlaying.svelte + CueDeck.svelte + Waveform.svelte + backend.ts + nativeBackend.ts), `scan/`, `settings/` (SettingsOverlay.svelte — Audio, Library (paths, scan and library health), Now Playing, Advanced tabs), `health/` (LibraryHealth.svelte), `toolbar/`, `track/` (TrackTooltip.svelte + MetadataOverlay.svelte + CuePointOverlay.svelte)

## Key Patterns

**Audio playback:** in-process Rust decks. The playlist loads tracks onto the main deck; the worker thread decodes via symphonia and emits time/duration/pause-state/ended/error events. No browser `<audio>` element, no `media://` protocol, no transcoder.

**Program bus:** every on-air deck is a `Sink` on one shared `OutputStream` mixer, driven by a single worker thread, so two decks can be audible at once (the precondition for handover). Deck events are **role-mapped**: whichever deck holds `main` emits `main-deck:*`, the armed one emits `arm-deck:*` — the renderer, broadcast service and now-playing webhook never learn which physical deck is on air. Roles are static today (slot A is `main`); handover is a later increment. The cue deck is deliberately off the bus with its own stream, thread and `cue:*` topics. See `docs/program-bus.md`.

**Cue points:** five nullable per-track markers (`cue_in_ms`, `fade_in_ms`, `fade_out_ms`, `cue_out_ms`, `next_start_ms`) stored as columns on `tracks`, deliberately absent from `UPSERT_TRACK_SQL` so a rescan cannot destroy them. `Cmd::Load` carries concrete `CuePoints`; the worker resolves them against the _decoded_ duration (the tag one is wrong on VBR MP3) and plays `take_duration(cueOut − pos)`, so the existing `sink.empty()` → `:ended` path ends a trimmed track with no new termination rule. Everything crossing the Tauri boundary is **air time**, measured from `cue_in` — a trimmed track is simply a shorter track to the renderer. Stored fades are a **source-level** envelope (`audio/envelope.rs`), not a `sink.set_volume()` ramp: a live fade and a stored fade would otherwise fight over one value, whereas a source envelope times a sink gain composes by multiplication. A track with no ramps is handed to the sink unwrapped. Clamping is backend-owned: `set_cue_points` returns what it stored, and `shared/cuePoints.ts` only ever applies the `null` fallbacks — there is no TypeScript clamp. See `docs/cue-points.md`.

**Durations mean air time.** Library rows, playlist rows, the history list, both decks and the now-playing webhook all report `cueOut − cueIn`, via `airDuration()`. So do the Upcoming tab total and the main deck's remaining-on-air countdown, both of which stop at the first stop marker. The toolbar's library _Playtime_ alone stays file time. `TrackTooltip` carries the file length too when the two differ. The renderer never optimistically shows file time, because the deck reports air time and the two would disagree mid-load.

**Authoring cue points.** `CuePointOverlay.svelte` (library row → _Cue points…_, or the cue deck's marker button) is the editor: `Waveform.svelte` with draggable handles plus a millisecond field per marker. The cue deck auditions in two modes that never mix — _Absolute_ plays the whole file so an in-point can be scrubbed for, _Preview_ reloads with markers applied and crops the waveform to the aired region. Switching reloads the deck because markers are applied at load time. `cue_load` takes an optional `cuePoints`, so the editor can audition an unsaved draft, plus an `autoplay` flag — the deck parks its sink when the background read lands, so a `Play` sent alongside a `Load` is undone by it. Cueing and mode switching stay **parked**; the editor's _Audition_ button is the only explicit ask. Auditioning happens inside the dialog (transport, playhead, click-to-seek on the curve), and the dialog borrows the cue deck and restores it on every exit, so no draft is left armed behind a closed one. Three labelled exits: _Save to track_ (radio edit, every airing), _Use once_ (queues that one airing), and _Cancel_, which asks before discarding. Saving a radio edit deliberately does **not** touch `currentTrack`: it applies from the next airing, so on-air audio never re-decodes under the operator.

**Item overrides.** A backend playlist item carries `cue_override: Option<CuePoints>` — cue points for that one airing. `None` means the item _references_ the track, so a corrected radio edit reaches every queued airing of it; an all-`NULL` override is distinct and means "play the whole file this once". `Effect::Play`/`Resume` carry the override because the item is consumed before the service runs the effect, and `prev` returns the outgoing track to the queue as an item, which is how the renderer-owned playlist's override-stripping defect stays fixed. The editor's _Use once_ is where one is authored — it queues the track next-up carrying the draft and never writes to the track; promotion from the cue deck attaches one too, but only when what is applied there differs from the radio edit. The playlist row's marker badge clears it. Both the queued overrides and the one on air are in `session.json`.

**Naming:** "edit" means metadata and nothing else (`MetadataOverlay.svelte`, `app.editingMetadata`); playback markers are always "cue points" (`CuePointOverlay.svelte`, `app.editingCuePoints`).

**Metadata edits.** `tracks.edited_fields` flags each tag column the operator changed (title 1, artist 2, album 4, genre 8, year 16). `UPSERT_TRACK_SQL` keeps a flagged column, so a rescan of a changed file cannot clobber the edit. The duplicate path copies the flags, and `revert_track_tags` re-reads the file and clears them. With `tuning.library.writeTags` on, `TagWriter` writes the edit into the file: tag in memory, check the fingerprint, write a temp file, rename it over the original. Never in place, since lofty truncates and rewrites in place. Success stores the new mtime and clears the flags. Failure keeps both and lists the write in the health report. See `docs/library.md#editing-a-track`.

**Search:** FTS5 virtual table on title/artist/album/genre. Triggers keep FTS in sync with tracks table. Query tokenized as prefix match: `foo bar` → `"foo"* "bar"*`.

**Scan + prune:** A scan never deletes a track. It lists every configured path, then applies one `Db::reconcile` transaction. Changed files are re-tagged. Rows whose file is gone get `missing_since`, but only under a fully listed root or outside every root — an unreachable or partly unreadable root marks nothing. New paths **reattach** to a missing row with the same fingerprint, or **duplicate** a present one (copying its operator state), or are inserted. Missing rows are hidden everywhere but stay readable by id; only _Settings → Purge_ deletes them. Root membership is `Path::starts_with`, never `LIKE`. A first scan into an empty library skips fingerprinting and leaves it to the background pass. See `docs/track-identity.md`.

**Library health:** `library::health::Health` keeps one report — missing tracks, exact duplicates (shared fingerprint), possible duplicates (music with the same normalised artist and title), unreadable tracks (`analysis_error`, set by the analysis pass when a file cannot be decoded and cleared by the upsert when the file changes; a read failure is not recorded) and the latest library check — and re-emits it as `library-health` after scans, the analysis pass, metadata edits, path changes and purges. The app never deletes audio files: an unwanted copy is deleted by the operator, then marked missing by a scan and purged with `purge_tracks(ids)`. Dismissals (`health_dismissals` table; the check's in memory) silence the badge only while the finding is unchanged. The playlist engine takes missing ids from the same event: advancement drops them up to the next stop marker, even on a cold cache. The library check runs at launch and every `tuning.library.checkIntervalMin` minutes, never alongside a scan, and reports only. See `docs/library-health.md`.

**Playlist ownership:** the backend owns the playlist, what is on air, and advancement. The renderer sends `playlist_*` commands and mirrors the `program:playlist-state` snapshot that comes back — it keeps no playlist of its own. History is the one exception: a renderer-side display log fed by each snapshot's `displaced` track. See `docs/backend-owned-playlist.md`.

**Auto-playlist:** Toggle mode that keeps a lookahead buffer queued, refilling from random DB selection when it drops below the threshold. Runs in `playlist::engine` alongside advancement, so a refill and the track change that triggered it are one transition.

**Seek:** `audio/player.rs` reloads the source on seek. `append_span` seeks in two stages — `try_seek` (container-level binary search) to ~200 ms short of the target, then `skip_duration` (sample iteration) for the remainder — so a stored marker lands sample-exactly even where symphonia estimates the seek by bitrate. A failing `try_seek` falls back to `skip_duration` from zero. `seek_offset + sink.get_pos()`, less `cue_in`, keeps `{role}:time` accurate.

## Gotchas

- `tauri::generate_context!()` runs at compile time and validates `frontendDist=../dist`. `cargo clippy` / `cargo test` panic with "frontendDist path doesn't exist" unless `pnpm vite build` has run; CI does this in `rust.yml` before cargo steps.
- `serde(default)` per-field on `SessionState` / `AppConfig` lets new fields land without a schema version bump. Match this pattern when adding fields.
- DB schema changes: append a step to `MIGRATION_STEPS` (never edit a shipped one), add a `SEEDS` entry, and regenerate `src-tauri/src/library/schema.sql` with `UPDATE_SCHEMA=1 cargo test schema_matches_snapshot`. Operator-work columns stay out of `UPSERT_TRACK_SQL`'s `SET` list. See `docs/database.md`.
- pnpm `minimumReleaseAge` constraint blocks plugin versions younger than ~3 days; pin to a slightly older stable version when adding `tauri-plugin-*` deps.
- Tauri command argument name `state` collides with the `State<AppState>` injection; the managed state arg is named `app` in command handlers.
- `release-please-config.json` bumps `package.json`, `src-tauri/tauri.conf.json` (jsonpath `$.version`), and `src-tauri/Cargo.toml` (`# x-release-please-version` annotation) on each release. Keep all three in sync.
- `tauri-plugin-log` is initialized first in the builder chain so panics before later plugin setup still reach the file sink. Renderer `console.*` is intercepted by `attachConsole()` in `main.ts`; vitest must not import `main.ts` (it doesn't — tests use `mockBackend`). Log level honors `RUST_LOG` (whole-app level only — no module syntax) and falls back to `Debug` in `cfg!(debug_assertions)` / `Info` in release. `symphonia*` modules are forced to `Warn` to keep the webview console readable.

## Data files

RadiodioDJ stores its database (`radiodiodj.db`), config (`config.json`), session
state (`session.json`), and default now-playing output in a per-user data
directory. Database backups sit beside it: `radiodiodj.v{N}.bak.db` (before a
migration, newest two kept) and `radiodiodj.legacy-v{N}.bak.db` (a pre-baseline
library that was reset).

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
