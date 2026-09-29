# External library and scanning from another computer

**Partly built.** Option A1 below ships as _Scan when files change_, described
for the operator in
[library-health.md](./library-health.md#scanning-by-itself); everything else here
is a design, not a description. The page answers whether the library can live
somewhere other than the studio machine, and whether a scan can be started — or
run — from another computer. Tracked as
[#495](https://github.com/Arskah/radiodiodj/issues/495). The library as it exists
today is [library.md](./library.md); its schema is [database.md](./database.md).

The ask behind the issue is one thing: **get library scans off the studio
machine.** That splits into two answers with an order of magnitude between them.
A _remote trigger_ leaves the scan where it is and lets something elsewhere start
it — and the cheapest version of that turns out to need no trigger at all, because
the app already watches the disk. An _external library_ moves the library onto a
server, disables the local database's owning functions, and moves the scan there
altogether.

## Today

Seven properties of the current build decide what is cheap and what is not.

**One local SQLite file, opened at launch.** `Db::open`
(`src-tauri/src/library/db.rs`) opens `radiodiodj.db` in the per-user data
directory (`src-tauri/src/lib.rs`), in WAL mode, behind a single
`Mutex<Connection>`:

```rust
pub struct Db {
    conn: Mutex<Connection>,
```

**Nothing abstracts the storage.** `db.rs` is some 7 400 lines of rusqlite with
the SQL written inline across about seventy methods. There is no trait, no enum,
no second implementation and no seam where one could be dropped in.

**The schema is SQLite to its bones.** An FTS5 external-content index with three
synchronising triggers, partial indexes, BLOB columns for the waveform and the
level envelope, schema versioning through `PRAGMA user_version` with an epoch in
`PRAGMA application_id`, and pre-migration backups taken with `VACUUM INTO`. See
[database.md](./database.md).

**The file cannot go on the share.** WAL needs a shared-memory mapping beside the
database, which network filesystems do not provide, and SQLite's locking is
unreliable over SMB and NFS in the first place. This is not a performance
objection to be measured — it does not work.

**Paths are absolute and canonicalised.** `Config::add_path`
(`src-tauri/src/persist/config.rs`) stores a canonical path per library root, and
`tracks.path` holds a canonical absolute path per file. The same share is
`/Volumes/radio/music` on macOS, `Z:\music` on Windows and `/mnt/radio/music` on
Linux, so one database cannot describe it for two machines.

**The database is on the on-air path.** Loading a track reads it:
`PlaylistService` calls `get_track_load_info` and `get_paths_by_ids`
(`src-tauri/src/playlist/service.rs`) as the playlist advances. Everything else
about playback is built so a dead share cannot take the station off air — whole
files into RAM, retries, a watchdog ([audio.md](./audio.md),
[playlist.md](./playlist.md)). A library that lives only on the network would
undo that, from the one direction the design has not defended.

**The scan is local by construction.** `scanner::scan_all` walks the roots on
four workers (`SCAN_CONCURRENCY`, `src-tauri/src/library/scanner.rs`), and the
analysis pass then **decodes every file** on `cores - 2` workers, capped at six
(`MAX_CONCURRENCY`, `src-tauri/src/library/waveform_scan.rs`) — reading them one
at a time, because the share cannot take the fan-out the CPU can. That decode is
the expensive part, and it is what an operator wants off the studio machine.

**The disk is already polled.** The library check
(`src-tauri/src/library/check.rs`) lists every root on a timer — fifteen minutes
by default — and reports what the next scan would change: `new`, `changed`,
`gone`, `unrooted`, plus the roots it could not read. It never writes to the
library. The worker already holds `ScanState`, already has an on-demand
`request()`, and each report already carries a `signature()` that identifies
_what_ was found rather than when. See
[library-health.md](./library-health.md#disk-changes).

Two more facts shape the options. There is **no inbound network surface**
anywhere in the app — `reqwest` is a client, for the outbound now-playing webhook
([now-playing-broadcast.md](./now-playing-broadcast.md)), and no server crate is
in `Cargo.toml`. And there is **no headless mode**: one Tauri app, everything
wired in `setup` against an `AppHandle`, with the scan and analysis jobs emitting
their progress to a window.

## Option A — let the machine start its own scan

The studio machine is already looking. Every fifteen minutes it lists the share
and works out exactly which files are new, changed or gone, and then does nothing
with the answer until an operator presses a button. Two variants follow from
that, and the first one needs no trigger, no protocol and no rendezvous.

### A1 — scan when the check finds changes (built)

`library.scanOnChanges` in `tuning`, **off by default**, shown as _Settings →
Library → Scan when files change_. With it on, a check whose report has changes
starts a scan through the same `ScanState::start` path the `scan_libraries`
command uses. The rules below are `LibraryCheck::may_scan` and
`settled_with_changes` in `src-tauri/src/library/check.rs`; what an operator sees
is [library-health.md](./library-health.md#scanning-by-itself).

Then "triggering a scan from another computer" is: copy music onto the share from
wherever you are. Within an interval the studio machine notices and brings itself
up to date. Nothing to install on the other computer, nothing to authenticate, no
control directory, no port.

The design is mostly guards, because a scan is the thing that writes to the
library.

- **It reverses a documented decision.** _"The check never starts a scan"_, under
  the rule that nothing in library health changes the library by itself. The
  setting is the operator's opt-out of that rule, which is why it is off by
  default and why both pages say so.
- **It adds and updates; it does not retire.** Marking tracks missing on the
  strength of a listing nobody watched is the one thing here that could empty a
  library — a stale mount that came back as an empty directory lists perfectly
  happily. A gone row is retired only when the same audio arrives elsewhere in
  the same scan, which is a move, and which has to be retired in the same
  transaction or the scan mints a duplicate. Everything else waits for the
  operator's button.
- **Wait for the disk to settle.** A copy in progress reports the same _paths_
  every time it is looked at, so a path list cannot tell a finished file from a
  growing one — and an in-place overwrite would read identically for the whole
  write. The settle key hashes each new or changed file's path, mtime and size,
  and two consecutive checks must agree on it. Cost: one extra interval of
  latency, so roughly half an hour on the default fifteen.
- **Never on a bad listing.** Any root `unreachable`, or any listing `partial`,
  and the check reports as before and starts nothing. A flapping share must not
  drive a scan loop.
- **Never twice on the same evidence.** A file no tag reader can parse writes no
  row, so it is reported as new forever; without this the share would be
  rescanned every two intervals for good.
- **Never over the operator.** A dismissed report and a cancelled scan both hold
  an automatic scan back.
- **The analysis pass, not the tag backfill.** An automatic scan owes the library
  the same waveforms and cue points the button does, but a cancelled backfill was
  cancelled on purpose and nothing the operator stopped should restart because a
  file appeared.

What A1 cannot do is force a scan on demand — it reacts to the disk, and reacts
one interval late. For that, A2.

### A2 — a scan request file on the share

The station already has one thing every machine can reach and write: the share.
Use it as the rendezvous.

A new `library.scanRequestPath` in `tuning`, **empty by default** so the feature
is off until an operator names a directory. Not a library root — the app never
writes into the music tree, and a root may be read-only.

The library check's worker already wakes at least once a minute, whatever the
check interval is set to:

```rust
/// How often a disabled timer looks at its setting again.
const IDLE_POLL: Duration = Duration::from_secs(60);
```

On each wake it stats one file in that directory. A request file whose mtime is
newer than the last honoured one starts a scan through the same
`ScanState::start` path the `scan_libraries` command uses. No new thread, no new
timer, no new dependency: a `touch` from another computer reaches the studio
machine within about a minute.

The honoured mtime is remembered in `config.json` rather than the file being
deleted. A relaunch must not re-fire a stale request, and the app then needs no
write permission on the request itself.

Feedback goes back the same way. On every `scan-state-changed` transition the app
writes a status file in that directory, carrying the `ScanStatus` the renderer
already receives (`src-tauri/src/library/scan_state.rs`) — so the remote side
polls a file instead of the app needing a reply channel.

Two things to say plainly:

- **This bypasses admin mode.** `scan_libraries` is in `ADMIN_COMMANDS`
  (`src-tauri/src/admin.rs`), so in the UI a scan needs the password. A file
  trigger's authority is write access to that directory, and nothing else. Put
  it where only trusted machines can write. See
  [admin-mode.md](./admin-mode.md).
- **A request mid-show is honoured.** Someone asked for it, at a moment they
  chose. The scan's four workers and the analysis pass's reserved cores are
  already sized to run under playback, the decks read whole files into RAM rather
  than streaming, and _Cancel_ stays in reach in the studio.

A1 and A2 share their machinery and stack cleanly: A1 covers "new music arrived",
A2 covers "scan now, I am not waiting for the interval". A1 is the smaller change
and the one that answers the issue's actual need; A2 is worth building only if
waiting an interval turns out to be the complaint.

What neither fixes: the decode still burns the studio machine's cores and its
share bandwidth. They move the button, not the work. That ceiling is what
makes option B worth pricing rather than dismissing.

## Option B — external library mode

A setting that says the library lives on a server: the local database's owning
functions go quiet, and the scan runs there. Two variants, and the difference
between them is most of the cost.

### B1 — an external SQL server the app connects to

Postgres or MySQL, with both the studio machine and the scanning machine
connecting to it. The work:

- A storage trait over ~70 `Db` methods, and every statement rewritten.
- FTS5 replaced — `tsvector` or `pg_trgm` — along with the triggers that keep it
  in sync. [library-search.md](./library-search.md) would have to be re-decided
  against a different engine.
- The epoch/`user_version` migration scheme replaced, and `VACUUM INTO` backups
  with it. [database.md](./database.md)'s rules are all SQLite's.
- A server to install, secure and back up, for a station that today installs one
  desktop app.

And when it is done, every track load crosses the network on the on-air path. B1
buys the most and defends the least.

### B2 — a library server of our own, with a local replica

One RadiodioDJ runs headless as the **library owner**. It keeps SQLite exactly as
today, owns the share, runs the scan, the analysis pass and the library check,
and exposes an API. Studio machines keep a **local replica** of that database and
read it through the existing `Db` code, unchanged.

That last part is the point. A dead server costs the studio machine library
freshness, never playback, because reads never left the machine. The abstraction
boundary lands at the app's own method surface instead of at SQL, so FTS5, the
migrations, the backups and every read path survive intact.

**What "disables local DB functions" means, concretely.** Server-owned in this
mode: `scan_libraries`, `cancel_scan`, `cancel_analysis`, `recalculate_auto_cue`,
`purge_tracks`, the health dismissals, the analysis pass, the tag backfill, the
tag writer and the check timer. Still local: session, devices, playlist,
appearance, and the play history the studio generates — play counts and the
airing log are written locally and pushed ([rotation.md](./rotation.md)).

Operator work on a track sits between the two. `set_cue_points` and
`update_track_metadata` are studio work, so they are sent to the server and
**refused with a clear error while it is unreachable**. Writing them into the
replica would diverge a copy that the next pull overwrites, and cue points are
exactly the work that must not be lost ([cue-points.md](./cue-points.md)).

**Increments.** Each is a PR on its own, and the page says which stand alone,
because increment 3 landing does not mean the feature works.

1. **Root-relative track paths.** `tracks.path` becomes a library-root id plus a
   path relative to it, and each machine maps root id → local mount in its own
   config. Touches the migration, `listing`, `scanner::scan_all`, `reconcile`,
   the library check and every path read. **Stands alone, and is worth doing
   regardless**: today a mount point that changes name loses the library, and
   every identity rule in [track-identity.md](./track-identity.md) is keyed on a
   path this would reshape.
2. **A `Library` boundary.** The `Db` surface split into the reads and writes the
   app actually calls, so a remote implementation can exist beside the local one.
   Mechanical, no behaviour change, and testable by itself. **Stands alone.**
3. **Headless mode.** Running the scan, the analysis pass and the check with no
   window, against a local SQLite as today. Needs the jobs' `AppHandle` emits put
   behind an event sink (`scan_state.rs`, `waveform_scan.rs`, `health.rs`). Useful
   on its own for bulk analysis and for tests — but **not a solution by itself**:
   two installs pointed at one share still have two unrelated libraries.
4. **The transport.** An HTTP server on the owner (no server crate is in the tree
   today), a station token, replica pull — a snapshot, then increments by a
   watermark — and a queue for the local writes that have to reach the owner.
5. **Client mode in the UI.** The external-library setting, the owning controls
   disabled with a reason given, a freshness indicator for the replica, and the
   error surfaces for an owner that cannot be reached.

## Rejected

| option                                              | why not                                                                                                                                                                                              |
| --------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| The SQLite file itself on the share                 | WAL needs shared memory and network locking is unreliable. Not a tuning question — it does not work.                                                                                                 |
| An HTTP listener in the studio app, just to trigger | An inbound port on the on-air machine, a token to manage, firewall and NAT, and a new dependency, to save a minute over option A. Worth it only if a status page or sub-minute triggering is wanted. |
| Scheduled nightly scans                             | A scan at a fixed hour, changes or not. A1 is the same idea with a better condition: scan because the disk moved, not because the clock did. Worth adding only as a quiet-hours window _around_ A1.  |
| A filesystem watcher                                | Already refused in [library-health.md](./library-health.md#not-built): the timer covers every share, and a watcher would only make local paths report sooner. A1 changes nothing about that.         |

## Verdict

**A1 is built. A2 if waiting an interval turns out to be the complaint. Option B
held.**

A1 was one setting and a handful of guards on a timer that already ticked and
already knew the answer. No schema change, no dependency, no second process, and
nothing to install on the other computer — the operator copies music onto the
share, and the studio machine catches up by itself. Its price is the reversal it
makes explicit: library health stops being a thing that only reports. Its limit
is latency, two intervals in the worst case.

A2 buys immediacy for a control directory, a status file, an mtime remembered in
`config.json` and a plain statement that the admin lock does not cover it. That
is a fair trade, but only against a complaint nobody has made yet.

Option B is the answer if that limit starts to hurt — if the library grows past
what the studio machine should be decoding between shows, or if a second studio
appears. Take **B2**, not B1: keeping SQLite local and replicating it is the only
variant that does not put the network on the on-air path, and it leaves the
schema, the search index and the migration rules alone. Increments 1 and 2 are
worth doing whether or not the rest ever ships, and increment 1 is the
prerequisite for any future in which two machines describe one share.
