# Documentation

Design and reference documentation for RadiodioDJ. The
[README](../README.md) is the operator's page (features, install, data files);
[AGENTS.md](../AGENTS.md) is the working index for anyone changing the code;
[CONTEXT.md](../CONTEXT.md) is the domain glossary. This folder is where the
_why_ lives.

Everything here describes what is built, except where a row says otherwise.

## Architecture

| doc                                  | answers                                                                             |
| ------------------------------------ | ----------------------------------------------------------------------------------- |
| [architecture.md](./architecture.md) | Where does a feature live? What crosses the Tauri boundary, and in which direction? |
| [database.md](./database.md)         | What is the schema, and how is it changed without breaking an operator's library?   |

## Audio and playback

| doc                                            | answers                                                                                 |
| ---------------------------------------------- | --------------------------------------------------------------------------------------- |
| [audio.md](./audio.md)                         | How is a file read, decoded, levelled and sent to a device? What is air time?           |
| [program-bus.md](./program-bus.md)             | How are two decks audible at once, how does handover work, what do the fade buttons do? |
| [cue-points.md](./cue-points.md)               | The five markers, how they are authored, and what actually airs.                        |
| [cue-auto-analysis.md](./cue-auto-analysis.md) | How markers are derived from a decode for a library nobody has cue-prepped.             |
| [tempo.md](./tempo.md)                         | How BPM is measured from the same decode, and why it sits beside the tag.               |

## Library

| doc                                      | answers                                                                                    |
| ---------------------------------------- | ------------------------------------------------------------------------------------------ |
| [library.md](./library.md)               | The whole library feature: paths, content types, scanning, tracks, editing.                |
| [track-identity.md](./track-identity.md) | Why a track survives being moved, renamed or re-added.                                     |
| [library-health.md](./library-health.md) | Missing tracks, duplicates, unreadable files, and the timed library check.                 |
| [library-search.md](./library-search.md) | **Planned.** Why search cannot forgive a typo today, and the fuzzy pass that would fix it. |

## Programming the station

| doc                          | answers                                                                     |
| ---------------------------- | --------------------------------------------------------------------------- |
| [playlist.md](./playlist.md) | Who owns the playlist, how it advances, and what an item override is.       |
| [rotation.md](./rotation.md) | The airing log, and the rules that stop the same artist coming round again. |

## Operating the app

| doc                                                    | answers                                                             |
| ------------------------------------------------------ | ------------------------------------------------------------------- |
| [admin-mode.md](./admin-mode.md)                       | What a password locks, and what stays available while it is locked. |
| [theming.md](./theming.md)                             | Writing a theme, the token contract, and station identity.          |
| [now-playing-broadcast.md](./now-playing-broadcast.md) | The outbound webhook and file output for stream overlays.           |

## Release

| doc                        | answers                                                         |
| -------------------------- | --------------------------------------------------------------- |
| [signing.md](./signing.md) | Code signing and notarization per platform, and what is set up. |
