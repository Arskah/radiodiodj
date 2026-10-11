# Shared library

One library on several computers. Every machine keeps its own SQLite library,
one of them owns the scan, and a Postgres **hub** sits between them: an owner
[publishes its library to the hub](#the-hub), a studio
[copies it](#applying-a-pull), and operator work — cue points, metadata edits,
hidden tracks, saved playlists, dismissals — made on any of them reaches the
rest. The other shapes considered, and why not them, are in
[#505](https://github.com/Arskah/radiodiodj/issues/505).

It stands on one thing already there: a track is stored relative to a library
path whose folder is each machine's own setting
([library.md](./library.md#where-a-library-path-is)).

## Roles

One setting with three values, under _Settings → Shared Library_, admin only.
It is read at launch, so a change takes a restart: a role is never changed
under a running playlist. It is the `externalLibrary` section of `config.json`:

```json
"externalLibrary": {
  "role": "owner",
  "url": "postgresql://radiodiodj:secret@hub.example.org:5432/radiodiodj",
  "machineName": "Office"
}
```

`machineId` appears beside them the first time the app starts in a role, and is
what the hub knows this install by from then on. A role with no address is
`standalone`, and so is a role this build does not know — the alternative is
the whole of `config.json` failing to parse over one word.

The page also says where the running role stands: the last thing the worker
found, when the hub last answered, whether that connection was encrypted, and
how many of this machine's changes have not been sent yet — on their way while
the hub answers, held while it does not. A hub that cannot be reached, or a role
this machine cannot play — an owner refused because another machine is the
owner, a studio at a hub nobody has published to — puts a mark on the Settings
button and on the tab, as a library health finding does. A studio also says
when nothing is feeding it: the owner has not checked in for five minutes, or
no machine holds the role.

| role         | scans and decodes | reads                  | talks to the hub                  |
| ------------ | ----------------- | ---------------------- | --------------------------------- |
| `standalone` | yes               | its own SQLite         | never — today's app, unchanged    |
| `owner`      | yes               | its own SQLite         | publishes the library, takes work |
| `studio`     | **no**            | its own SQLite, a copy | takes the library, sends work     |

A station that wants the decode off the on-air box installs a second
RadiodioDJ as the `owner` and makes the on-air one a `studio`.

Nothing on the on-air path changes with the role. Search, the playlist, the
decks and the rotation rules read the local database through today's `Db`, and
the hub is touched by one worker that nothing else waits on.

```mermaid
flowchart LR
  subgraph owner["Owner"]
    S["Scan, analysis pass, check"] --> ODB[("SQLite")]
    ODB -- "triggers" --> OC["sync_rows"]
    OC <--> OW["Sync worker"]
  end
  HUB[("Postgres hub")]
  subgraph studio["Studio"]
    P["Playlist, search, decks, editors"] <--> RDB[("SQLite")]
    RDB -- "triggers" --> SC["sync_rows"]
    SC <--> SW["Sync worker"]
  end
  OW <--> HUB
  SW <--> HUB
```

## What travels

Replication is by **group**, not by table row: a group is the smallest thing two
machines can disagree about, and each has one rule. A group is named by a `kind`
and a `key`, and crosses the hub as one JSON document.

| kind        | key                | document                                                            | written by  | when two disagree            |
| ----------- | ------------------ | ------------------------------------------------------------------- | ----------- | ---------------------------- |
| `root`      | root id            | content type                                                        | owner       | cannot happen                |
| `track`     | track id           | everything the scan and the analysis pass write, with the two BLOBs | owner       | cannot happen                |
| `cue`       | track id           | the two fades, and the trio once it is `manual`                     | any machine | later save wins, for the set |
| `edit`      | track id and field | one edited tag column's value                                       | any machine | later save wins, per field   |
| `hidden`    | track id           | when it was hidden                                                  | any machine | later one wins, per track    |
| `playlist`  | the list's `uid`   | name and every entry, in order                                      | any machine | later save wins, per list    |
| `dismissal` | finding kind, key  | the value it was dismissed at                                       | any machine | later one wins, per finding  |

Every column of `tracks` belongs to exactly one of these, with two deliberate
exceptions.

**The trio is in two groups, and ownership says which.** Cue In, Cue Out and
Next Start travel in the `track` document while `auto_cue_state` is `auto`, and
in the `cue` document once it is `manual`. A machine applying a `track` document
leaves the trio alone on a row it holds as `manual` — the guard `set_auto_cue`
already has inside one database
([cue-auto-analysis.md](./cue-auto-analysis.md)), applied across the hop.
`manual` is permanent, so the two can never trade places and no clock is needed
to settle it.

**An edited tag column is in two groups, and the flag says which.** The `track`
document carries every tag column as the owner's file reads; a machine applying
it keeps a column whose `edited_fields` bit is set, which is the rule
`UPSERT_TRACK_SQL` applies to a rescan. The edit itself travels as an `edit`
group. A revert is an `edit` group marked deleted: the bit clears and the
column follows the `track` document again.

**Hiding a track is a group of its own.** `hidden_at` is one admin's decision
about one track, and it is neither a cue set nor a tag, so it is stamped and
settled by itself: hiding on one machine does not lose a cue save made on
another in the same minute. Unhiding is the same group with no time in it, not
a tombstone — the track is still there.

`play_count` is in none of them, and neither is the airing log. See
[What stays on the machine](#what-stays-on-the-machine).

## Change capture

The same work whether or not a hub is ever configured. It is one migration
step and nothing else: no `Db` method changes.

```sql
CREATE TABLE sync_rows (
  kind      TEXT NOT NULL,
  key       TEXT NOT NULL,
  edited_at INTEGER NOT NULL,            -- unix ms, the saving machine's clock
  machine   TEXT,                        -- NULL: saved on this machine
  deleted   INTEGER NOT NULL DEFAULT 0,  -- a tombstone
  pending   INTEGER NOT NULL DEFAULT 1,  -- not sent yet
  PRIMARY KEY (kind, key)
) WITHOUT ROWID;

CREATE TABLE sync_local (
  id            INTEGER PRIMARY KEY CHECK (id = 1),
  capture       INTEGER NOT NULL DEFAULT 0,  -- 0 while standalone
  applying      INTEGER NOT NULL DEFAULT 0,  -- 1 inside a pull's transaction
  library_id    TEXT,                        -- the hub library this copy belongs to
  pulled_rev    INTEGER NOT NULL DEFAULT 0
);
```

`sync_rows` is three things at once, which is why it is one table. It is the
**stamp** — the time and machine of a group's last edit. It is the **outbox** —
`pending` rows are what this machine still owes. And it is the **tombstone** —
a purged track, a deleted saved playlist and an undone dismissal leave a row
with `deleted` set where the data used to be.

Triggers write it (`library/db/sync.rs`): one per group on `tracks`, and one
each on `library_roots`, `saved_playlists` and `health_dismissals`. Each is
`AFTER UPDATE OF` its group's columns, so a waveform landing does not stamp a
cue set, and bumping a play count stamps nothing. Where two groups share a
column the trigger compares old with new: a trio moved by hand marks `cue` and
not `track`, an edited title marks its `edit` and not `track`.

`saved_playlist_entries` needs no trigger. Every operator change to a list's
entries already updates the list's `updated_at`, and that is what marks the
`playlist` group; binding an entry to a track is each machine's own derivation
and does not travel.

**The stamp is SQLite's clock**, read in the trigger, since a trigger can read
no other. It is the saving machine's wall clock either way. The merge rules are
tested by passing stamps in documents, not by pinning this one.

**The analysis commit can move a fade.** `set_auto_cue` sorts the stored fades
against the trio it writes, and when that changes one the `cue` group is marked
with the `track`: the stored set did change, and it travels as any other save
does. An analysis that moves no fade marks `track` alone.

Two guards sit in every trigger's `WHEN`:

- **`capture`** is off for a standalone library, so the triggers write nothing
  and "inert" is literal. Taking a role turns it on and marks every existing
  group pending in one statement.
- **`applying`** is set and cleared inside the transaction that applies a pull,
  so applying another machine's change is not captured as one of this
  machine's. `Db` is one `Mutex<Connection>`, so nothing else can write while
  it is set, and a crash rolls it back with the transaction.

The same step adds `saved_playlists.uid` — a random identifier, set by a trigger
on insert and backfilled for existing rows. `id` stays a local `AUTOINCREMENT`
and is what `session.json` and the auto-playlist source keep naming; `uid` is
what the hub knows the list by. Without it two studios would each create list
`7`.

**A column nobody claimed never travels.** A column added to `tracks` after this
lands in no trigger's list unless someone puts it there, and the failure is
silent: the studios simply never see it. So a test reads the column list of
`tracks` and fails unless each column is named in exactly one group or in the
short list of local-only ones — the contract-guard pattern the theme tokens use.

## The hub

Three tables, created and migrated by the owner.

```sql
CREATE TABLE hub_station (
  id         boolean PRIMARY KEY DEFAULT true CHECK (id),
  library_id text    NOT NULL,   -- new whenever the owner's library is replaced
  protocol   integer NOT NULL,
  owner      text,               -- the one machine that may write root and track
  settings   jsonb   NOT NULL    -- unused
);

CREATE TABLE hub_machines (
  id text PRIMARY KEY, name text NOT NULL, build text NOT NULL,
  seen_at timestamptz NOT NULL
);

CREATE TABLE hub_rows (
  kind      text    NOT NULL,
  key       text    NOT NULL,
  rev       bigint  NOT NULL,
  machine   text    NOT NULL,
  edited_at bigint  NOT NULL,
  deleted   boolean NOT NULL DEFAULT false,
  doc       jsonb,
  waveform  bytea,               -- track only
  levels    bytea,               -- track only
  PRIMARY KEY (kind, key)
);
CREATE INDEX hub_rows_rev ON hub_rows (rev);
```

`hub_rows` is deliberately generic. The hub does not know what a track is: a
column added to `tracks` changes a JSON document and no hub table.

**A push** is one transaction. It takes `pg_advisory_xact_lock` on the station,
then upserts each pending group with a fresh `rev` from one sequence:

- `root` and `track` are written only when the pushing machine is
  `hub_station.owner`, unconditionally.
- every other kind is written only where
  `(edited_at, machine)` is greater than what the hub holds. A push that loses
  is not an error, and its answer carries the save it lost to, which the
  pushing machine applies: it may have pulled that save already and then saved
  over it with an earlier clock, and no pull would bring it again.
- a `cue`, `edit` or `hidden` group for a track whose tombstone the hub holds
  is not written at all.

The lock is what makes `rev` usable as a cursor. Without it two writers can
commit out of order, and a reader that has already passed `rev` 41 never sees a
40 that commits late. Pushes are small and rare, so serialising them costs
nothing worth measuring.

**A pull** reads `hub_rows` above `sync_local.pulled_rev`, in
`rev` order, in pages, and applies each page in one local transaction with the
cursor.

**One owner.** A machine in the `owner` role claims `hub_station.owner` on each
visit, and is refused while another machine holds it: it publishes nothing and
says who the owner is. Every publish checks the claim again inside its
transaction. Taking the role over from a machine that is gone is done in the
hub: clear `hub_station.owner` by hand.

**A machine that becomes a studio gives the role up.** Its claim would
otherwise name an owner that never publishes again.

**One library.** `library_id` is stamped when the owner first publishes, and
kept in its `sync_local`. An owner that finds the hub holding any other id —
its own database was replaced, or it was pointed at a hub it never published
to — **empties `hub_rows`**, stamps a new id and publishes everything again:
the rows there carry another database's track ids. A hub with no station gets a
new id as well, even from an owner that had one: a hub that was made again
counts its revisions from one, and a cursor kept from the old one would step
over everything in it. A studio whose
`sync_local.library_id` differs stops syncing and says it has to join again.

**The version rule.** `protocol` is a constant in the build, and the hub carries
the owner's. A machine that does not know the hub's protocol keeps its database,
keeps playing, stops syncing and says so. Documents are keyed by column name, so
inside one protocol a key a build does not know is ignored and one it expected
is left at its default.

**The client** is `tokio-postgres` over `rustls`, which `reqwest` already brings
in (`hub/`). It runs in one worker task and nowhere else: every 30 seconds it
connects, claims, checks in to `hub_machines`, and publishes what `sync_rows`
owes in transactions of 200 groups. Whether the connection is encrypted is the
URL's `sslmode` to say. Every hub call has a timeout, and none holds the `Db`
mutex. What it finds goes to the log, once per change and not once per visit.
Between visits it looks at its own `sync_rows` every two seconds and visits at
once when something is waiting, so an operator's save does not wait half a
minute — unless the last visit could not send, because it failed, another
machine is the owner or the hub holds no library, when the hub is left alone
until the next one is due.

**Both roles send, then take.** The owner sends the library and its own operator
work, then takes what studios sent — leaving out its own rows, which would
otherwise be the whole library handed back. A studio sends its operator work
and takes everything. The protocol number is 2 from here: a build that knew
only `root` and `track` would step over the other kinds and never see them
again, so it has to stop instead.

**The hub's address is a connection URL in `config.json`, password included, in
plain text.** It is the only secret there that is not a hash, and the README's
data-files section says so. What bounds it is the Postgres role: a studio
connects as `studio`, which can move rows and cannot change the hub's shape.

## Reaching the hub

The owner and every studio connect out to the hub, often across the internet,
with a password. So the connection is **encrypted unless an operator says
otherwise**, and the hub has to prove who it is: a certificate from an
authority this machine trusts, for the host name in the address. Both are
checked whenever TLS is used; there is no setting that encrypts without
checking, because one that exists gets switched on.

Three ways a station runs its hub, and what each needs under _Settings → Shared
Library → Connection_:

| the hub                                                           | certificate from   | needs                                         |
| ----------------------------------------------------------------- | ------------------ | --------------------------------------------- |
| is reached through a proxy that ends TLS — an ingress, a balancer | a public authority | _Encryption ends at a proxy_                  |
| holds its own certificate, as a database operator sets it up      | the cluster's own  | _Hub's own certificate authority_: its `.crt` |
| is on a private network or behind a tunnel, with no TLS           | —                  | _Allow an unencrypted connection_             |

**Trust is the system's authorities plus one of the station's.** A public
certificate needs nothing configured. A private authority is chosen once per
machine, and trusted for the hub only, beside the system's and not instead of
them. Its certificate is public, so `config.json` keeps the PEM text itself
(`caCertificate`) and not a path to a file somebody tidies away; the page shows
whose it is and when it expires.

**A proxy that ends TLS has to see the handshake first.** Postgres' own way in
opens with a plaintext question — "do you speak TLS?" — before any handshake,
which a proxy waiting for a handshake never answers. `directTls` skips the
question (`sslnegotiation=direct`). The proxy's certificate is usually a public
one, which makes this the deployment that needs nothing installed on a studio.
A database that holds its own certificate takes the handshake first only from
PostgreSQL 17 on.

**Plaintext is a choice, never a default.** TLS is tried first, always. Only
when the hub answers that it has none, and `allowUnencrypted` is on, is the
connection made without it — so allowing plaintext never costs an encryption
the hub offers, and a hub whose certificate cannot be trusted is refused, not
talked to in the clear. The page says which of the two the last connection was.
A `sslmode` in the URL is not what decides this; the two settings are.

**A refusal says what to do.** An unknown authority asks for the hub's CA
certificate, a certificate for another name says so, an expired one says so,
and a hub with no TLS asks for it to be enabled or for plaintext to be allowed.

Not done: client certificates. A machine proves who it is with its password.

## Applying a pull

All of this is one module that takes documents and a `Db`, so its rules are
tested against two in-memory SQLite databases with no Postgres in sight.

| kind        | applied as                                                                                                            |
| ----------- | --------------------------------------------------------------------------------------------------------------------- |
| `root`      | inserted or removed with the owner's id. The folder is not in it — a new root is _Not located on this computer_.      |
| `track`     | written with the owner's id, keeping edited columns and a manual trio. A tombstone deletes as a purge does.           |
| `cue`       | written unless this machine holds a `pending` cue edit for the track that is later. Fades are clamped again on write. |
| `edit`      | the column and its `edited_fields` bit, unless a later `pending` edit of that field waits here.                       |
| `hidden`    | `hidden_at` set or cleared, unless a later `pending` one waits here.                                                  |
| `playlist`  | the list replaced whole, found by `uid`. Entries are bound by track id, then by fingerprint as an import does.        |
| `dismissal` | upserted, or deleted by a tombstone.                                                                                  |

**A pull never overwrites a later save waiting to go out.** A visit sends
before it takes, and a group that arrives while this machine has its own
version waiting is compared with it the way the hub compares: the later
`edited_at` stands, and the machine id settles a tie. Ours later — the incoming
one is passed over, and the hub takes ours next. Theirs later — it is applied
and ours is no longer owed. Every machine and the hub reach the same answer
from the same two stamps, which is what makes the order of arrival not matter.
A group that arrives with the very stamp this library holds is passed over: a
studio is handed its own saves back, since a copy that was made again needs
them, and they are not changes.

**Work that arrives before its track waits for it.** The hub holds each group as
it reads now and hands them out in the order they last changed, so a cue set can
come a page ahead of a track that was updated after it. A `cue`, `edit` or
`hidden` group for a track this library does not hold is kept in `sync_parked`
and applied when the track is; a purge of the track drops it. The owner removes
a purged track's groups from the hub with the tombstone.

**What applying changed is passed on**, since the library is not the only place
it lives: the playlist re-reads the cue points of what it holds — after a
`track` as after a `cue`, since the automatic ones travel in the track — the list of
saved playlists is emitted again, and on the owner a track another machine
edited is handed to the tag writer, which writes it to the file if _Write edits
to file tags_ is on.

**A reverted edit says what it went back to.** Only the owner reverts, since
the file is its to read, and the `edit` tombstone it sends carries the column's
value after the revert — so a studio needs nothing but the tombstone to follow.

**Two saved playlists may share a name.** A name is checked on create and
rename on one machine, and two machines can each make _Friday_. Both arrive and
both stay; they are different lists by `uid`, and either can be renamed.

**A purge on the owner reaches a studio as a delete.** It runs the same cleanup
`purge_tracks` does — `play_log.track_id` nulled, saved-playlist entries
unbound — but is not that method, which deletes missing rows only.

## What stays on the machine

The hub shares a library. It does not make two studios one radio station, and
nothing about what a machine put on air crosses it.

- **The airing log.** `play_log` is written and read where the track aired.
  History is this machine's, and so are the rotation rules: what another studio
  played is not something this one avoids ([rotation.md](./rotation.md)).
- **Play counts.** `play_count` is a local column in no group. The owner's is
  not sent, and a studio's counts what that studio aired.
- **The playlist, the session and every device and tuning setting.** That
  includes the automatic cue settings: the owner's thresholds shape the cue
  points every machine is given, and whether they are applied on air is each
  machine's own switch.

So no `Db` query changes, and `play_log` needs no column saying where an airing
came from. A machine that joins as a studio starts its log and its counts
empty, with the library it replaces; the old file keeps the old ones.

## Joining and leaving

**Becoming the owner** keeps the library. Capture is switched on, every group
is marked pending, and the first push publishes it.

**Becoming a studio replaces this machine's library**, because its track ids
are its own and the owner's are the station's. It works as the baseline reset
does ([database.md](./database.md#baseline-resets)): the old file is kept, as
`radiodiodj.standalone.bak.db` (a later join never writes over it, and is kept
as `standalone.2.bak.db`); the ids in `session.json` are dropped; and the
toolbar says why the library is empty until the hub has answered. Like that
reset it happens **at the next launch**, never under a running playlist —
Settings asks before saving the role — and the library is empty until the
first pull finishes, so joining is done off air. A library that holds nothing
is not set aside, only marked.

What says a database is a studio's copy is `sync_local.replica`, so a studio
keeps its copy from one launch to the next and a hand-edited role cannot fill
a library that was never set aside.

**The library paths arrive unlocated.** A studio gets the owner's paths by id
and content type; where each is on this machine is _Locate_, under _Settings →
Library_, as for any path whose folder this computer has not been told
([library.md](./library.md#where-a-library-path-is)). Until then its tracks
are listed and cannot be loaded.

**Joining again.** A studio whose hub now holds a different library stops and
says so. Setting the role to _Not shared_, restarting, and setting it back
joins from the start.

**A studio owes the hub what it does, and nothing it was given.** Capture
starts with an empty outbox on a copy: everything in it came from the hub, and
sent back stamped with today it would beat the saves it came from.

**Leaving** keeps the copy and turns capture off. The machine is a standalone
station with the library it last pulled.

## What a studio does not run

The role gates the jobs, not only their buttons.

- The scan, _Scan when files change_, the analysis pass, the tag backfill, the
  library check and its timer, _Recalculate now_, purge, and adding or removing
  a library path. The jobs are never started, and their commands are refused
  with the reason, as admin mode refuses a locked one: the list is
  `hub::OWNER_COMMANDS`, checked by the same wrapper. Their controls are
  disabled with that reason as the tooltip.
- The tag writer. A metadata edit made on a studio reaches the owner as an
  `edit` group, and the owner writes it to the file if _Write edits to file
  tags_ is on there.
- Reverting a metadata edit to the file's tags. The file is the owner's to
  read: a studio whose path is located at the wrong folder, or at a stale copy,
  would send another file's tags to every machine. A studio undoes an edit by
  editing it back.
- The second half of `set_mounts`. _Locate_ works — the folder is this
  machine's — but re-deriving which root each track is under does not run:
  `root_id` and `path` are the owner's columns.
- Forgetting a dismissal whose finding is gone, which `health::build` does on
  every report. A studio that is behind would otherwise delete a dismissal the
  owner still needs and send the tombstone to everyone.
- Reclassifying a track. Its content type follows its library path, so a
  change made here would be put back by the owner's next scan; the command
  drops it on a studio.

## What goes wrong

| failure                                   | what happens                                                                                  |
| ----------------------------------------- | --------------------------------------------------------------------------------------------- |
| The hub is unreachable                    | Nothing on air notices. Edits wait in `sync_rows`; the copy grows stale.                      |
| The owner is off                          | Studios still exchange cue points, edits and saved playlists. No new tracks arrive.           |
| A studio was off for a month              | It pulls from its cursor. Nothing is kept per machine in the hub, so there is no log to trim. |
| Two machines edit one cue set             | The later save wins, by the saving machines' clocks.                                          |
| Analysis lands on a track cued by a hand  | The manual trio stands on every machine. Order of arrival does not matter.                    |
| A track is purged while a studio holds it | The tombstone deletes it there; an edit the studio had pending for it is dropped by the hub.  |
| The owner's database is restored or reset | A new `library_id`. Studios stop and ask to join again.                                       |
| A studio's build is too old               | It keeps playing from its copy, stops syncing and says so.                                    |
| A second machine is made the owner        | Refused, and told who the owner is.                                                           |

## Testing

- **Capture** — `cargo test`, in-memory SQLite: each `Db` write marks exactly
  the groups it should, and none while `capture` is off or `applying` is on.
- **Merge rules** — `cargo test`, two in-memory databases and the documents
  passed by hand.
- **The hub** — a Postgres service in CI, where each test makes a schema of its
  own. A local run without `RADIODIODJ_TEST_HUB` set to a connection URL passes
  them without running.
- **TLS** — a second Postgres in CI, started with a certificate from an
  authority made for the run: `RADIODIODJ_TEST_HUB_TLS` is its URL and
  `RADIODIODJ_TEST_HUB_CA` the authority's certificate. It is version 17, so
  the handshake-first test has a server that takes one.
