# Architecture

RadiodioDJ is a [Tauri 2](https://tauri.app) desktop app with two process
boundaries: a **Rust backend** and a **Svelte 5 / Vite renderer**, talking over
Tauri `invoke` (renderer → backend, request/response) and `emit` / `listen`
(backend → renderer, fire-and-forget).

The split is not cosmetic. Everything that must be right while audio is on air —
decoding, the mixer, what plays next, when the next track starts — lives in
Rust, on threads that a busy webview cannot stall. The renderer is a control
surface and a projection of backend state; it holds no playback state of its
own.

## Where a feature lives

| feature                           | backend                                              | renderer                              | doc                                                    |
| --------------------------------- | ---------------------------------------------------- | ------------------------------------- | ------------------------------------------------------ |
| decode, devices, levels           | `audio/`                                             | `features/deck/`                      | [audio.md](./audio.md)                                 |
| what a decode measures            | `audio_measure/`                                     | —                                     | [audio-measure.md](./audio-measure.md)                 |
| on-air mixing, handover, fades    | `audio/bus.rs`, `audio/deck.rs`                      | `features/deck/NowPlaying.svelte`     | [program-bus.md](./program-bus.md)                     |
| cue points and their editor       | `audio/cue_points.rs`, `audio_measure/auto_cue.rs`   | `features/track/Cue*.svelte`          | [cue-points.md](./cue-points.md)                       |
| the library and scanning          | `library/`                                           | `features/library/`, `features/scan/` | [library.md](./library.md)                             |
| track identity across moves       | `audio_measure/fingerprint.rs`, `library/listing.rs` | —                                     | [track-identity.md](./track-identity.md)               |
| library health                    | `library/health.rs`, `check.rs`                      | `features/health/`                    | [library-health.md](./library-health.md)               |
| the playlist and advancement      | `playlist/`                                          | `features/playlist/`                  | [playlist.md](./playlist.md)                           |
| rotation rules and the airing log | `library/db.rs`, `playlist/generate.rs`              | —                                     | [rotation.md](./rotation.md)                           |
| now-playing output                | `broadcast/`                                         | `features/settings/`                  | [now-playing-broadcast.md](./now-playing-broadcast.md) |
| themes and station identity       | `appearance/`                                        | `shared/appearance.ts`                | [theming.md](./theming.md)                             |
| admin mode                        | `admin.rs`                                           | `features/admin/`                     | [admin-mode.md](./admin-mode.md)                       |
| schema and migrations             | `library/db.rs`                                      | —                                     | [database.md](./database.md)                           |

## Rust backend (`src-tauri/src/`)

Grouped by domain.

- **`lib.rs`** — `tauri::Builder` setup (`tauri-plugin-log` first, then
  `tauri-plugin-dialog`), `AppState`, every `#[tauri::command]` handler, and the
  panic hook that force-captures a backtrace into `log::error!`.
- **`main.rs`** — a thin `pub fn main() { radiodiodj_lib::run() }` binary entry.
- **`admin.rs`** — admin mode: the `AdminLock` (unlocked flag, Argon2 password
  check), the `ADMIN_COMMANDS` list, and the gate the invoke handler applies.
- **`audio/`** — playback: the decode path, output devices, decks, the program
  bus, the cue deck, cue points, the fade envelope and the ReplayGain setting.
  Its own code map is in [audio.md](./audio.md#code-map).
- **`audio_measure/`** — what a decode says about a track: the waveform curve,
  the loudness numbers, the level envelope behind the automatic cue points, the
  tempo, and `fingerprint.rs`'s tag-independent content hash of the first MiB of
  demuxed packets. A library, not a feature — it opens no device, reads no
  setting and touches no database, and `library/waveform_scan.rs` is the pass
  that calls it. See [audio-measure.md](./audio-measure.md).
- **`library/`** — `db.rs` (rusqlite + FTS5, WAL, `parking_lot::Mutex<Connection>`,
  append-only `rusqlite_migration` steps against a `schema.sql` snapshot,
  pre-migration backup, newer-DB refusal), `scanner.rs` + `scan_state.rs`
  (recursive `walkdir` scan, `lofty` tags, an mtime + content-type delta cache,
  one reconcile transaction per scan, a background worker emitting
  `scan-progress` / `scan-state-changed` with a cancel token), `listing.rs`
  (root listing plus the changed/gone rules, shared by scan and check),
  `waveform_scan.rs` (the background analysis pass filling waveforms,
  fingerprints, loudness and automatic cue points), `tag_backfill.rs` (the
  background pass filling tag columns a row predates), `health.rs`, `check.rs`
  (timed, listing-and-stat only, never writes) and `tag_write.rs` (opt-in
  write-back of metadata edits).
- **`playlist/`** — the playlist and everything that advances it: `generate.rs`,
  `engine.rs` (the pure state machine), `service.rs` (effects → deck commands)
  and `model.rs` (wire types). See [playlist.md](./playlist.md).
- **`broadcast/`** — the now-playing output: `state.rs` (what is currently on
  air), `payload.rs` (the template substitution), `webhook.rs`, `file_sink.rs`
  (atomic write) and `service.rs`.
- **`appearance/`** — `theme.rs` (the `Theme` model, `THEMEABLE_TOKENS`, the
  colour-value grammar and `validate`) and `store.rs` (enumerating
  `{app_data_dir}/themes`, first-run seeding of the copy-me `example/`,
  built-ins via `include_str!`, base merge, active-theme resolution, and
  station-identity images under `{app_data_dir}/branding/`).
- **`persist/`** — `config.rs` (`AppConfig` → `{app_data_dir}/config.json`) and
  `session.rs` (`SessionState` → `{app_data_dir}/session.json`), both with
  per-field `serde(default)`.

## Renderer (`src/`)

Vite root, following the Tauri Svelte template convention.

- **`main.ts`** — app entry. Mounts Svelte, attaches the log console bridge, and
  hooks Tauri `onCloseRequested` to await `flushSave()` before `win.destroy()`.
- **`App.svelte`** — the top-level UI tree.
- **`shared/`** — `types.ts`, `api.ts` (a typed `invoke()` wrapper plus the
  folder picker via `@tauri-apps/plugin-dialog`), `state.svelte.ts` (the Svelte 5
  `$state` store: deck transport through `DeckTransport`, playlist state mirrored
  from backend snapshots), `cuePoints.ts` / `cueEditor.ts`, `appearance.ts`,
  `health.ts`, plus the colocated `state.test.ts`, `mockBackend.ts` and
  `mockPlaylist.ts`.
- **`features/<feature>/`** — one folder per UI feature: `library/`, `playlist/`,
  `deck/` (NowPlaying, CueDeck, Waveform, and the `backend.ts` /
  `nativeBackend.ts` transport), `scan/`, `settings/` (SettingsOverlay, with
  Audio, Library, Playlist, Now Playing, Appearance and Advanced tabs),
  `health/`, `admin/`, `toolbar/`, `track/` (the tooltip, the metadata overlay
  and the cue-point editor) and `ui/`.

### The projection rule

Backend state reaches the renderer as **whole snapshots**, never deltas, and the
renderer never computes a value the backend already owns. The playlist snapshot
(`program:playlist-state`) and the health report (`library-health`) both work
this way: idempotent, and a dropped or reordered event cannot desynchronise the
UI from what is actually going to air.

The renderer may hold optimistic UI state for something the operator is doing
right now (a volume drag, a seek in flight), but it is overwritten by the next
snapshot rather than merged with it.

## Events

Deck events are **role-mapped**. Whichever deck holds the `main` role emits
`main-deck:*`, the armed one `arm-deck:*`, an outgoing one `tail-deck:*`. The
renderer, the broadcast service and the now-playing webhook never learn which
physical deck is on air. The cue deck, being off the bus, emits `cue:*`.

| topic                                                                                                                                  | from                 | carries                                     |
| -------------------------------------------------------------------------------------------------------------------------------------- | -------------------- | ------------------------------------------- |
| `{role}:time` (10 Hz), `:duration`, `:pause-state`, `:ended`, `:loaded`, `:load-failed`, `:buffering`, `:error`, `:output-unavailable` | a deck               | transport state on the air timeline         |
| `cue:*`                                                                                                                                | the cue deck         | the same set, off air                       |
| `program:roles`                                                                                                                        | the bus              | the slot → role map                         |
| `program:handover`                                                                                                                     | the bus              | the role move the engine reconciles against |
| `program:faded-out`                                                                                                                    | the bus              | a completed fade to silence on `main`       |
| `program:playlist-state`                                                                                                               | the playlist service | the whole playlist snapshot                 |
| `library-health`                                                                                                                       | `library/health.rs`  | the whole health report                     |
| `scan-progress`, `scan-state-changed`                                                                                                  | the scan worker      | counts, current file, run state             |
| `cache-state`                                                                                                                          | the prefetch cache   | which track ids are resident in RAM         |

## Conventions

- **Commands are flat and `snake_case`** (`playlist_add`, `cue_load`,
  `set_cue_points`); events are `topic:verb-phrase`.
- **Tauri command arguments may not be named `state`** — it collides with the
  `State<AppState>` injection. The managed state argument is named `app`.
- **New config and session fields carry `serde(default)`**, so they land without
  a schema version bump.
- **Admin-gated commands** must be listed in `admin::ADMIN_COMMANDS`, or they run
  while admin mode is locked. See [admin-mode.md](./admin-mode.md).
- **Colours live only in `:root`.** A themeable UI has no hardcoded colour
  literals; see [theming.md](./theming.md#the-token-contract).

## Build shape

`pnpm dev` runs `tauri dev`: Vite with HMR for the renderer, `cargo watch` for
the backend. `pnpm build` produces platform bundles under
`src-tauri/target/release/bundle/`.

`tauri::generate_context!()` runs at compile time and validates
`frontendDist=../dist`, so `cargo clippy` and `cargo test` fail with
"frontendDist path doesn't exist" unless `pnpm vite build` has run first. CI
does this in `rust.yml` before any cargo step.

Signing and notarization are [signing.md](./signing.md).
