# RadiodioDJ

Desktop playout software for radio stations. Keep music, commercials and
jingles in separate libraries, cue them like a real rig, and let the station run
itself — or drive every transition by hand.

Built with Tauri 2 and Svelte 5, with an in-process Rust audio engine. No
browser audio pipeline, no external transcoder, no subscription.

## Features

### On air

- **Two-deck program bus** — decks are summed into one output, so two tracks can
  be audible at once. Transitions are a **handover** at the outgoing track's
  _Next start_ marker, not a fixed crossfade duration
  ([docs](docs/program-bus.md))
- **Live fades** — _Fade out_ ramps the on-air track to silence and stops it;
  _Fade to next_ starts the next item now and fades the outgoing one out
  underneath it ([docs](docs/program-bus.md#live-fades))
- **Cue deck** — audition any track on a second output device (headphones)
  without touching what is on air, then promote it to next-up
  ([docs](docs/audio.md#the-cue-deck))
- **Now playing** — waveform, elapsed and remaining time, cover art, and a
  progress bar that seeks on double-click (a single click cannot move an on-air
  track)
- **Playback modes** — AUTO advances through the playlist, MANUAL stops after
  each track. **Stop markers** park the show at a fixed point in the queue
- **ReplayGain levelling** — every track measured against one reference during
  analysis, so a 1970s master and a modern one sit at the same level
  ([docs](docs/audio.md#replaygain))
- **Native audio** — in-process Rust player (rodio + symphonia) for MP3, FLAC,
  Vorbis, WAV, AAC, M4A, Opus, AIFF and more ([docs](docs/audio.md))

### Cue points

- **Five markers per track** — cue in, fade in, fade out, cue out and next
  start, stored in the library and applied at every airing. The audio file is
  never modified ([docs](docs/cue-points.md))
- **Cue editor** — a two-strip waveform editor with draggable handles, keyboard
  nudging, pre-roll and live audition through the cue deck
  ([docs](docs/cue-points.md#authoring))
- **Automatic cue points** — silence-trimmed starts and ends, and a music segue
  point, derived from the analysis decode. An unprepared library airs tight
  without anyone touching a marker ([docs](docs/cue-auto-analysis.md))
- **Use once** — shorten a track for tonight's show only, without changing its
  stored radio edit ([docs](docs/playlist.md#item-overrides))

### Library

- **Three content libraries** — separate folders and browsing for music,
  commercials and jingles ([docs](docs/library.md))
- **Fast search** — full-text search across title, artist, album and genre
  (SQLite FTS5)
- **Stable track identity** — moving, renaming or re-adding files keeps each
  track's cue points, play count and edits
  ([docs](docs/track-identity.md#for-the-operator--moving-and-reorganising-files))
- **Metadata editing** — fix a title or artist in the app; a rescan cannot
  clobber the edit, and with write-back enabled it is written into the file's
  tags ([docs](docs/library.md#editing-a-track))
- **Library health** — missing tracks, duplicates, unreadable files and disk
  changes in one report, with a timed check that never touches the library
  ([docs](docs/library-health.md))
- **Network-share resilience** — tracks are read whole into RAM and prefetched
  ahead of the playlist, so a share that stalls mid-show does not stall the
  output ([docs](docs/audio.md#whole-file-reads))

### Programming

- **Auto playlist** — continuous playback that keeps a lookahead buffer queued
  and refills itself from the music library ([docs](docs/playlist.md))
- **Interleave** — a jingle every 4 music tracks and a commercial every 8, both
  configurable
- **Rotation rules** — never reselect a track, or an artist, that aired inside a
  configurable window, backed by a persistent airing log
  ([docs](docs/rotation.md))
- **History** — what actually aired, surviving restarts

### Station

- **Now-playing broadcast** — outbound webhook plus atomic file output for
  stream overlays, metadata bridges and scripted consumers
  ([docs](docs/now-playing-broadcast.md))
- **Themes** — colour schemes as a folder you can write yourself; two built in
  ([docs](docs/theming.md))
- **Station identity** — your station name, toolbar logo and record label on the
  deck vinyl ([docs](docs/theming.md#station-identity))
- **Admin mode** — a password that locks settings and destructive actions while
  leaving playback, cueing and browsing open to whoever is on shift
  ([docs](docs/admin-mode.md))

## Usage

1. Click **Paths** to configure folders for music, commercials and jingles
2. Click **Scan** to index audio files and extract metadata
3. Use the **library tabs** to browse by content type
4. Double-click a track or use **+** to add it to the playlist; right-click a
   row for cueing, cue points, metadata editing and play-now
5. Toggle **Auto Playlist** for continuous playback
6. Switch between **AUTO** and **MANUAL** playback modes

## Prerequisites

- [Node.js](https://nodejs.org/) >= 20
- [pnpm](https://pnpm.io/)
- [Rust toolchain](https://rustup.rs/) (stable)
- Linux only: `libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libssl-dev libasound2-dev`

## Development

```bash
pnpm install
pnpm dev          # tauri dev — Vite HMR for renderer, cargo watch for backend
```

## Build

```bash
pnpm build        # tauri build — produces platform bundles in src-tauri/target/release/bundle/
```

## Tests

```bash
pnpm test -- run                                  # vitest renderer tests
cargo test --manifest-path src-tauri/Cargo.toml   # cargo backend tests
pnpm typecheck && pnpm lint                       # tsc + svelte-check + eslint
```

## Data files

RadiodioDJ stores its database (`radiodiodj.db`), config (`config.json`), session
state (`session.json`), themes (`themes/`), station artwork (`branding/`), and
default now-playing output in a per-user data directory. Database backups sit beside it: `radiodiodj.v{N}.bak.db` (before a
migration, newest two kept) and `radiodiodj.legacy-v{N}.bak.db` (a pre-baseline
library that was reset).

- macOS: `~/Library/Application Support/com.radiodiodj/`
- Linux: `~/.local/share/com.radiodiodj/` (or `$XDG_DATA_HOME/com.radiodiodj/`)
- Windows: `%APPDATA%\com.radiodiodj\` (typically `C:\Users\<you>\AppData\Roaming\com.radiodiodj\`)

Forgot the admin password: quit the app, delete `passwordHash` from the `admin`
section of `config.json`, and relaunch. See [docs/admin-mode.md](docs/admin-mode.md).

## Logs

RadiodioDJ writes a rotating log file (`RadiodioDJ.log`, 1 MB max, one prior file kept).

- macOS: `~/Library/Logs/com.radiodiodj/RadiodioDJ.log`
- Linux: `~/.local/share/com.radiodiodj/logs/RadiodioDJ.log` (or `$XDG_DATA_HOME/com.radiodiodj/logs/`)
- Windows: `%LOCALAPPDATA%\com.radiodiodj\logs\RadiodioDJ.log`

Set `RUST_LOG=debug` (or `trace`) before launching to raise verbosity. Default is `info` (release) or `debug` (dev).

## Stack

- Tauri 2 + Svelte 5 + Vite (renderer)
- Rust backend: `rusqlite` (FTS5), `rodio` + `symphonia`, `lofty` for tag metadata, `walkdir` for filesystem scan, `parking_lot` for sync primitives
- Husky + lint-staged + ESLint + Prettier + Vitest

## Documentation

- [docs/](docs/README.md) — design and reference documentation, indexed by topic
- [AGENTS.md](AGENTS.md) — working index for changing the code
- [CONTEXT.md](CONTEXT.md) — the domain glossary

## License

[GNU General Public License v3.0 or later](LICENSE). Use it, run it at your
station, sell it — but if you distribute a modified RadiodioDJ, ship its source
under the same license. Patches back to
[this repo](https://github.com/Arskah/radiodiodj) are welcome.

## Sponsoring

RadiodioDJ is free and open source. If it keeps your station on air, you can
buy me a coffee:

[![Buy Me A Coffee][bmc-button]][bmc]

[bmc]: https://www.buymeacoffee.com/arska
[bmc-button]: https://cdn.buymeacoffee.com/buttons/v2/default-yellow.png
