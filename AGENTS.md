# AGENTS.md

## Build & Run Commands

```bash
pnpm dev                                          # tauri dev (Vite HMR for renderer, cargo watch for backend)
pnpm build                                        # tauri build → src-tauri/target/release/bundle/<format>/
pnpm typecheck                                    # svelte-check (--tsgo) + tsgo on tsconfig.node.json/e2e
pnpm test                                         # vitest watch
pnpm test -- run                                  # vitest single run
pnpm e2e                                          # tauri-driver + WebdriverIO (Linux only — see e2e/README.md)
cargo test --manifest-path src-tauri/Cargo.toml   # backend tests (db, scanner, session, playlist, audio, config)
pnpm lint                                         # eslint
pnpm format                                       # prettier --write .
pnpm format:check                                 # prettier --check .
```

## Architecture

Tauri 2 app. Two process boundaries: a Rust backend (`src-tauri/src/`, grouped
by domain — `audio/`, `library/`, `playlist/`, `broadcast/`, `appearance/`,
`persist/`, `admin.rs`) and a Svelte 5 / Vite renderer (`src/`, one folder per
UI feature under `features/` plus `shared/`), talking over Tauri `invoke` +
`emit`/`listen`.

The module map, the event table and the boundary conventions are
[docs/architecture.md](docs/architecture.md). Domain vocabulary is
[CONTEXT.md](CONTEXT.md). All design docs:
[docs/README.md](docs/README.md).

## Key patterns

Each entry is the invariant to preserve; the linked doc carries the reasoning.

**Audio playback** — in-process Rust decks, no browser `<audio>`, no `media://`,
no transcoder. A deck reads the **whole file into RAM** (retry + 10 s watchdog)
and never streams from the filesystem, which is what survives a wedged network
share. See [docs/audio.md](docs/audio.md).

**Program bus** — every on-air deck is a `Sink` on one shared `OutputStream`,
driven by one worker. Deck events are **role-mapped**: `main-deck:*`,
`arm-deck:*`, `tail-deck:*`. Nothing outside the bus learns which physical deck
is on air. Handover moves the `main` role at the outgoing track's `next_start`;
the engine authorises it by arm-loading and reconciles against
`program:handover`. At most two tracks are ever audible. The cue deck is
deliberately off the bus, on its own stream, thread and `cue:*` topics. See
[docs/program-bus.md](docs/program-bus.md).

**Live fades** — one primitive, `Cmd::Fade { to, ms, on_complete }`, stepped
from the bus worker's 50 ms tick. It **multiplies** the operator's deck volume
rather than replacing it, and every command that changes what a deck is doing
cancels it and restores full gain. `Cmd::HandOverNow` is the one command acting
on two decks, so the worker intercepts it before dispatch. A completed fade to
silence on `main` emits `program:faded-out`, which the service maps to the same
`stop()` the Stop button runs. Durations are read per press from
`tuning.player`. See [docs/program-bus.md](docs/program-bus.md#live-fades).

**ReplayGain** — measured by the analysis pass, never read from tags.
`Cmd::Load` carries an already-resolved linear factor, so the worker never
consults the library, and it is applied at the source in `append_span` (not
`sink.set_volume()`) and therefore before the bus mixer. `rg_measured_at` is
what "measured" means. No album mode. See
[docs/audio.md](docs/audio.md#replaygain).

**Cue points** — five nullable columns on `tracks`, deliberately absent from
`UPSERT_TRACK_SQL` so a rescan cannot destroy them. `Cmd::Load` carries concrete
`CuePoints`, resolved against the **decoded** duration. Stored fades are a
source-level envelope (`audio/envelope.rs`), never a sink ramp — a live fade and
a stored fade would otherwise fight over one value. Clamping is backend-owned:
`set_cue_points` returns what it stored, and `shared/cuePoints.ts` only applies
`null` fallbacks. See [docs/cue-points.md](docs/cue-points.md).

**Durations mean air time** — everything crossing the boundary reports
`cueOut − cueIn` via `airDuration()`; position `0` is the first audible sample.
The toolbar's library _Playtime_ is the one exception. Never optimistically show
file time. See [docs/audio.md](docs/audio.md#durations-mean-air-time).

**Automatic cue points** — cue in, cue out and (for music) next start derived
from the waveform pass's RMS windows, no second decode. Ownership is a per-track
state (`pending` / `auto` / `manual`); an operator write that moves the trio
makes it `manual` for good, and `set_auto_cue` only commits while the track is
still automatic. Re-analysis is scheduled for `auto` tracks only; changing a
threshold never re-analyses anything. The decode is also reduced to one byte per
RMS window into `auto_cue_levels` — the levels that window is above — written
with the trio in one statement, so a later threshold change can re-derive
markers without reading the file again, and a later rule can ask the
measurement something new without a fresh decode. That is why thresholds round
to whole decibels. _Recalculate now_ (`recalculate_auto_cue`) is the only thing
that applies new thresholds to existing material: rows with a readable envelope
are re-derived from it, the rest go back to the analysis pass, and `manual` rows
are counted and skipped. Two nested switches
decide what the library _reports_ — `autoCue.apply` over the whole trio, `autoCue.applyNextStart`
over the Next Start alone — and both gate in `effective_cue_points`, so nothing
downstream of the library knows they exist. See
[docs/cue-auto-analysis.md](docs/cue-auto-analysis.md).

**Authoring cue points** — `CuePointOverlay.svelte` is the only surface that
moves a marker, with the rules in `shared/cueEditor.ts`. The editor's
input-only neighbour stop is the one exception to backend-owned clamping. The
dialog borrows the cue deck and restores it on every exit, so no draft is left
armed behind a closed one. Saving a radio edit deliberately does **not** touch
`currentTrack`. See [docs/cue-points.md](docs/cue-points.md#authoring).

**Item overrides** — `cue_override: Option<CuePoints>` on a playlist item.
`None` means the item _references_ the track; an all-`NULL` override is distinct
and means "play the whole file this once". Effects carry the override because
the item is consumed before the service runs them, and `prev` returns the
outgoing track to the queue as an item. See
[docs/playlist.md](docs/playlist.md#item-overrides).

**Playlist ownership** — the backend owns the playlist, what is on air,
advancement, refill, the prefetch window and history. The renderer sends
`playlist_*` commands and mirrors the whole `program:playlist-state` snapshot;
it computes nothing. Snapshots are whole, never deltas. See
[docs/playlist.md](docs/playlist.md).

**Auto-playlist** — a lookahead buffer refilled inside `playlist::engine`, so a
refill and the track change that triggered it are one transition. Interleave
counters advance on music only. See [docs/playlist.md](docs/playlist.md#modes).

**Rotation rules** — both predicates run in SQL (`SelectionFilter` in
`library/db.rs`), so a query returns exactly the count asked for; the queue
counts as already aired. One track per artist per generated block via
`ROW_NUMBER() OVER (PARTITION BY artist ...)`. A short block refetches the
deficit down a three-rung ladder, warning per relaxation; the id exclusion is
never relaxed. Jingles and commercials are untouched by both rules. See
[docs/rotation.md](docs/rotation.md).

**Seek** — the source is reloaded, then `append_span` seeks in two stages:
`try_seek` to ~200 ms short, then `skip_duration` for the remainder, so a marker
lands sample-exactly. See [docs/audio.md](docs/audio.md#seek).

**Search** — FTS5 virtual table on title/artist/album/genre, kept in sync by
triggers. A query is tokenized as prefix match: `foo bar` → `"foo"* "bar"*`. See
[docs/library-search.md](docs/library-search.md) for the planned fuzzy pass.

**Scan + prune** — a scan never deletes a track. One `Db::reconcile` transaction
per scan. Rows whose file is gone get `missing_since`, but only under a fully
listed root or outside every root. New paths **reattach** by fingerprint,
**duplicate** a present row (copying its operator state), or are inserted. Root
membership is `Path::starts_with`, never `LIKE`. Only _Settings → Purge_
deletes. See [docs/track-identity.md](docs/track-identity.md).

**A changed file is not changed audio** — an external tagger, `touch` and
`rsync` all move an mtime without touching a sample. The scan re-fingerprints a
known path whose mtime moved and the fingerprint decides: same → the tags are
re-read and **nothing measured is disturbed**; different → a different track,
so the row goes missing and the file enters through the new-path ladder
(`Reconcile::replaced`); unknown → the row stands and its measurements are
dropped. Every measurement in `UPSERT_TRACK_SQL` hangs on
`excluded.fingerprint = fingerprint`, whose `NULL` propagation _is_ the unknown
case. The automatic trio hangs on `excluded.content_type = content_type` too,
because a reclassified root rescans files whose audio never moved and a
music-derived Next Start must not stand on a jingle; the level envelope
deliberately does not, being a measurement of audio no reclassification
touched. A measurement is also refused at the other end: `set_waveform`,
`set_loudness` and `set_fingerprint` check the `mtime` the decode read, so a
file replaced mid-decode cannot land a stale result over the invalidation.
Never invalidate a measurement on the mtime alone. See
[docs/library.md](docs/library.md#tracks).

**Library health** — one report (missing, exact and possible duplicates,
unreadable tracks, the latest library check), re-emitted as `library-health`
after scans, the analysis pass, metadata edits, path changes and purges. The app
never deletes audio files. Dismissals silence the badge only while the finding
is unchanged. The playlist engine takes missing ids from the same event. The
check reports only and never runs alongside a scan. See
[docs/library-health.md](docs/library-health.md).

**Metadata edits** — `tracks.edited_fields` flags each operator-changed tag
column and `UPSERT_TRACK_SQL` keeps a flagged column, so a rescan cannot clobber
an edit. `TagWriter` never writes in place (lofty truncates and rewrites): tag in
memory, check the fingerprint, write a temp file, rename it over the original.
See [docs/library.md](docs/library.md#editing-a-track).

**Theming** — a theme sets **colours only**; the token contract is the `:root`
block of `src/styles.css`. Validation is a token-name allowlist plus one closed
value grammar, run in Rust at load, so the renderer never sees an unvalidated
token. An invalid theme is refused whole; an incomplete one is filled from its
`base`. Station identity is configured separately and wins over theme images.
Nothing repaints unasked — no watcher, no following the OS. See
[docs/theming.md](docs/theming.md).

**Admin mode** — `lib.rs` wraps the command handler in `admin_gated`, which
rejects every command in `admin::ADMIN_COMMANDS` while locked. The unlocked flag
lives only in `AppState`; the renderer mirrors it and runs the idle timer, which
can lock but never unlock. Not a security boundary. See
[docs/admin-mode.md](docs/admin-mode.md).

**Naming** — "edit" means metadata and nothing else (`MetadataOverlay.svelte`,
`app.editingMetadata`); playback markers are always "cue points"
(`CuePointOverlay.svelte`, `app.editingCuePoints`). The rest of the vocabulary
is [CONTEXT.md](CONTEXT.md).

## Gotchas

- A new colour token must land in three places — the `:root` block in
  `src/styles.css`, `THEMEABLE_TOKENS` in `appearance/theme.rs`, and every
  built-in theme JSON — or the contract guard fails. Colour literals outside
  `:root` fail it too. See [docs/theming.md](docs/theming.md#the-token-contract).
- `tauri::generate_context!()` runs at compile time and validates
  `frontendDist=../dist`. `cargo clippy` / `cargo test` panic with "frontendDist
  path doesn't exist" unless `pnpm vite build` has run; CI does this in
  `rust.yml` before cargo steps.
- `serde(default)` per-field on `SessionState` / `AppConfig` lets new fields land
  without a schema version bump. Match this pattern when adding fields.
- DB schema changes: append a step to `MIGRATION_STEPS` (never edit a shipped
  one), add a `SEEDS` entry, and regenerate
  `src-tauri/src/library/schema.sql` with
  `UPDATE_SCHEMA=1 cargo test schema_matches_snapshot`. Operator-work columns
  stay out of `UPSERT_TRACK_SQL`'s `SET` list. See
  [docs/database.md](docs/database.md).
- pnpm `minimumReleaseAge` constraint blocks plugin versions younger than
  ~3 days; pin to a slightly older stable version when adding `tauri-plugin-*`
  deps.
- Two TypeScript compilers are installed: `typescript` (6.x, the API
  typescript-eslint and svelte-check load) and `@typescript/native`
  (an npm alias for TypeScript 7, used by `tsc -p` runs and
  `svelte-check --tsgo`). `node_modules/.bin/tsc` is ambiguous between them, so
  the typecheck scripts spell out `node_modules/@typescript/native/bin/tsc`.
  `renovate.json` caps `typescript` below 7 until typescript-eslint supports it
  ([#10940](https://github.com/typescript-eslint/typescript-eslint/issues/10940)).
- `svelte-check --tsgo` writes transpiled Svelte files to `.svelte-check/` and
  never prunes them, so a deleted component keeps reporting its old errors. The
  `svelte-check` script wipes the directory first; re-running `svelte-check`
  directly needs the same wipe.
- A new admin-only command must be added to `admin::ADMIN_COMMANDS`, or it runs
  while admin mode is locked. See [docs/admin-mode.md](docs/admin-mode.md).
- Tauri command argument name `state` collides with the `State<AppState>`
  injection; the managed state arg is named `app` in command handlers.
- `release-please-config.json` bumps `package.json`,
  `src-tauri/tauri.conf.json` (jsonpath `$.version`), and `src-tauri/Cargo.toml`
  (`# x-release-please-version` annotation) on each release. Keep all three in
  sync.
- `tauri-plugin-log` is initialized first in the builder chain so panics before
  later plugin setup still reach the file sink. Renderer `console.*` is
  intercepted by `attachConsole()` in `main.ts`; vitest must not import
  `main.ts` (it doesn't — tests use `mockBackend`). Log level honors `RUST_LOG`
  (whole-app level only — no module syntax) and falls back to `Debug` in
  `cfg!(debug_assertions)` / `Info` in release. `symphonia*` modules are forced
  to `Warn` (`symphonia_bundle_mp3` to `Error`, whose false-sync warnings on a
  non-MP3 file otherwise fill the 1 MB log) to keep the webview console
  readable.

## Data files and logs

Per-user data directory paths, database backup naming, log file locations and
the admin-password reset are in the README:
[Data files](README.md#data-files), [Logs](README.md#logs).

## Testing

Pre-commit hook runs lint-staged (prettier + eslint fix) then vitest.

CI gates Rust with `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings`
(`.github/workflows/rust.yml`); run both locally before pushing.
