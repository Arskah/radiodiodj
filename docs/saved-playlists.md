# Saved playlists

A named, stored list of tracks: built before a show or kept as a curated pool,
moved between machines as a file, appended to the on-air playlist, or used as
what the auto-playlist draws from.

**Planned — nothing here is built.** Designed 2026-10-04 against
[#501](https://github.com/Arskah/radiodiodj/issues/501),
[#503](https://github.com/Arskah/radiodiodj/issues/503) and
[#577](https://github.com/Arskah/radiodiodj/issues/577). The library selection
([#576](https://github.com/Arskah/radiodiodj/issues/576)) is built on its own
and knows nothing of saved playlists; what it and dragging
([#584](https://github.com/Arskah/radiodiodj/issues/584)) add here comes after
increment 1.

## The problem

Nothing in the app stores a list. The database holds `tracks`, `play_log` and
`health_dismissals`; the only ordered sequence of tracks is the on-air playlist,
which lives in `session.json` and is consumed as it airs. So a show cannot be
prepared before its hour, a prepared show cannot be carried to the studio, and
the auto-playlist has exactly one pool: the whole music library.

The word is taken, too. **Playlist** in [CONTEXT.md](../CONTEXT.md) is the
on-air sequence, and every `playlist_*` command and `playlist/` module means
that. The stored thing is a **saved playlist** everywhere in code and docs. The
library tab that lists them is labelled _Playlists_, because that is what an
operator calls them.

## Model

A saved playlist is a name and an ordered list of **entries**. An entry refers
to one track of any content type. The same track may appear more than once: a
theme tune opens and closes a show.

It is never on air by itself. It reaches air one of two ways, both described
under [Using one](#using-one): its entries are appended to the playlist, or it
becomes the auto-playlist's source.

No stop markers. A stop marker is a playlist item, not a track, and the source
path below would have to ignore it anyway. An operator who wants the show to
park adds one to the playlist after appending.

### Storage

```sql
CREATE TABLE saved_playlists (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  name       TEXT NOT NULL,
  created_at INTEGER NOT NULL,  -- unix ms UTC
  updated_at INTEGER NOT NULL
);

CREATE TABLE saved_playlist_entries (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  playlist_id  INTEGER NOT NULL,
  position     INTEGER NOT NULL,
  track_id     INTEGER,          -- NULL while unmatched, or once purged
  fingerprint  TEXT,             -- snapshot: what the entry means
  artist       TEXT,
  title        TEXT,
  duration     REAL,
  content_type TEXT
);
CREATE INDEX saved_playlist_entries_list
  ON saved_playlist_entries(playlist_id, position);
```

One appended migration step; see [database.md](./database.md).

`track_id` is the local binding and it is what everything reads: a bound entry
shows the live track, so a metadata edit or a corrected radio edit reaches every
saved playlist the track is in. The snapshot columns are what the entry is when
nothing is bound. They are what an unmatched entry displays, and what a file
arriving later is matched against. This is the shape `play_log` already has, for
the same reason — see [rotation.md](./rotation.md#the-airing-log).

No foreign keys, also for the reason given there: `PRAGMA foreign_keys` is off.
Deleting a saved playlist deletes its entries in the same transaction, and
`Db::purge_tracks` nulls `track_id` here in the transaction that already does it
for `play_log`.

### Unmatched and missing

Two different states, one badge.

- An **unmatched entry** has no `track_id`: no track in this library answers to
  it. It came from a file, or its track was purged.
- An entry bound to a **missing track** has a `track_id` whose row carries
  `missing_since`. The file is gone; the track, its cue points and its play
  count are not. See [track-identity.md](./track-identity.md#missing-not-deleted).

Both are shown with the missing badge, both are kept, and neither can be
appended or picked. The second heals itself: a scan that reattaches the track
clears `missing_since` and the entry is playable again with no work here.

### Binding

`Db::bind_saved_entries` is one idempotent statement: every entry with no
`track_id` takes the present track with the same fingerprint. It runs

- at import,
- at the end of a scan's reconcile transaction, and
- when the analysis pass goes idle.

Both of the last two are needed. A fingerprint is written by the scanner for a
new or changed path, and by `Db::set_fingerprint` from the analysis pass for a
row that has none — and a first scan into an empty library is tag-only, so there
the pass is the only writer. See
[track-identity.md](./track-identity.md#fingerprint).

Two present tracks can share a fingerprint: an exact duplicate, or two masters
that share their first megabyte, which that page lists as an accepted risk. The
entry's `duration` breaks the tie, closest wins, lowest id after that.

**Text never binds.** An artist and title that match exactly one track are still
not the recording the author chose: a live version, a clean edit and a remaster
all read the same. This is the stance **possible duplicates** already take — a
notice for the operator, not a match. What an unmatched entry gets instead is
_Find in library_: the search box prefilled with its artist and title, and the
operator picks.

**A fingerprint version bump** re-fingerprints the library, which would strand
every stored snapshot at once. That is why `track_id` is the binding and the
snapshot is not: a bound entry never consults its own fingerprint, and export
writes the bound track's current one.

## The file

Export writes it; import reads it; so does anything else that can see the
library.

```json
{
  "format": "radiodiodj-playlist",
  "version": 1,
  "name": "Friday Rock",
  "entries": [
    {
      "fingerprint": "v2:…",
      "artist": "…",
      "title": "…",
      "duration": 214.0,
      "contentType": "music"
    }
  ]
}
```

No track ids and no paths. An id belongs to one install's database, and a path
to one machine's mount; the fingerprint is the only identity that travels.

Import **never drops an entry and never overwrites a saved playlist**. What does
not bind arrives unmatched and stays in its place in the order. A name already
in use gets a suffix. Replacing an existing saved playlist is an edit, and a
file dropped on the window by a guest must not be able to make one.

A `version` this build does not know is refused whole, with the reason — the
rule themes already follow.

### Where a file comes from

Two authors are planned for:

- **The app**, on the studio machine or another install pointed at the same
  library.
- **A web page** that works from a copy of the library, so a show can be built
  without a seat in the studio. The page does not exist, and its copy of the
  library is [external-library.md](./external-library.md) territory
  ([#505](https://github.com/Arskah/radiodiodj/issues/505)). The file is the
  contract it will be written against.

Both know fingerprints, which is what makes the fingerprint-only rule
affordable. M3U is not read: its identity is a path.

## Using one

### Appending to the playlist

Two actions on a saved playlist, because which one was pressed should be visible
in what happened:

- **Add to playlist** — every playable entry, in order, exactly as written.
- **Add with jingles and commercials** — the same list with the station's
  [interleave](./playlist.md#modes) woven through it: `interleave_evenly`, with
  jingle and commercial counts derived from the list's music entries and drawn
  from the full jingle and commercial libraries.

Both **append**. Nothing replaces the operator's playlist.

Either one is a single transition and a single snapshot. The bulk add that takes
is `Playlist::insert_many`, which
[#576](https://github.com/Arskah/radiodiodj/issues/576) brings for the library
selection as `playlist_add_many`. Unplayable entries are skipped, and the panel
reports "added 31, skipped 2 missing" instead of leaving the operator to count.
Where that count comes from is open: a playlist command queues its transition
and returns nothing ([playlist.md](./playlist.md#commands)), so it cannot be the
command's result as `playlist_add_many` stands.

This is also how a saved playlist airs **in order**. There is no in-order mode
of the auto-playlist: appending the whole show puts it in the Upcoming tab,
where it can be seen, reordered and trimmed, and the auto-playlist takes over by
its ordinary refill when the show runs out. A cursor in the engine would have
kept the playlist short at the price of hiding the rest of the show from the
person running it.

### As the auto-playlist's source

The **auto-playlist source** is where music selection draws from: the music
library, which is today's behaviour and the default, or one saved playlist.

It is one more predicate on the music pick. `SelectionFilter` gains the pool,
and `get_random_tracks` restricts to tracks some entry of that saved playlist is
bound to. Everything else in [rotation.md](./rotation.md) is untouched: both
windows, the queue counting as aired, the artist spread, the relaxation ladder.

`DbRefiller` is built by the service for each transition, from config; the
source rides in it beside `rotation`. The `Refiller` trait does not change and
the engine gains no behaviour. It carries the source as a value, into the
snapshot so the panel can show it and into `session.json` so it survives a
restart.

The rules around it:

- **Only music entries are the pool.** Jingles and commercials in the saved
  playlist are ignored here, and interleave draws from the full libraries as it
  always has. Commercials in particular must not be narrowed by a music
  curation: `pick_random_from_bottom` is what evens airings across advertisers.
- **A small pool repeats; it does not leak.** The queue is never relaxed, so a
  fifteen-track pool holds at most fourteen queued and settles into a loop, and
  the ladder logs every refill. That is what a small curated pool means, and
  topping up from the library would air tracks the operator left out on
  purpose. The panel says the pool is smaller than the rotation windows.
- **An empty pool reverts.** A source with no playable music — deleted, or every
  music entry missing or unmatched — would leave the auto-playlist switched on
  and adding nothing. The service checks before it builds the refiller; an empty
  source is cleared back to the music library and the snapshot carries a notice.
  Choosing an already-empty one is refused. Deleting the active source is
  allowed and ends the same way.
- **Choosing a source is not starting a show.** It sets the source and nothing
  else. `set_auto_playlist(true)` starts playback on an idle deck, and a row in
  the library panel must not put audio on air. `stop` turns the auto-playlist
  off and leaves the source alone.
- **Switching does not touch the playlist.** Twenty tracks from the old source
  are about an hour of audio, and the new one starts after them. Items carry no
  record of who queued them, so replacing "the generated ones" is not available,
  and clearing would discard what the operator queued by hand. The panel says
  how many queued tracks air first and offers the existing Clear beside it.
- **Edits apply at the next refill.** The pool is read from the table each time.

## Authoring

In the first increment, with a saved playlist open in the _Playlists_ tab:

- **Add to saved playlist ▸** in the track row's context menu, from any library
  tab, the playlist and history. Appends at the end.
- **Remove** an entry.
- **Drag** to reorder, the way playlist rows already do.
- **Save playlist as…** on the playlist panel: the upcoming tracks become a new
  saved playlist. Stop markers and item overrides are not carried.

Later, on top of the library selection
([#576](https://github.com/Arskah/radiodiodj/issues/576), which queues several
tracks to the playlist and stops there): the selection gains _Add to saved
playlist ▸_ and _New saved playlist from selection_, and the entries of an open
saved playlist become selectable.
[#584](https://github.com/Arskah/radiodiodj/issues/584) adds dragging from the
library.

## Admin mode

The line [admin-mode.md](./admin-mode.md) draws is that a guest runs a show and
cannot change the station. Here that reads: a guest can **make** a saved
playlist and **use** any of them, and cannot change one that exists.

| open while locked                               | admin only                      |
| ----------------------------------------------- | ------------------------------- |
| import, _Save playlist as…_, new from selection | add, remove and reorder entries |
| export                                          | rename, delete                  |
| both append actions                             | _Find in library_               |
| choosing the auto-playlist source               |                                 |

So for a guest, creation is atomic — a whole list in one action. There is no
"the person who made it may keep editing it": the app has no idea who made
anything, and ownership is a larger feature than the one it would serve here.
The show a guest wants to adjust is adjusted in the playlist, where it already
is theirs.

## Wire

Commands are flat and do I/O, so each is an `async fn` over one `blocking(…)`
call ([architecture.md](./architecture.md#commands)):

```
saved_playlist_get          saved_playlist_create       saved_playlist_import
saved_playlist_export       playlist_add_saved          playlist_save_as
playlist_set_source
```

and, listed in `admin::ADMIN_COMMANDS`:

```
saved_playlist_add_entries  saved_playlist_remove_entry saved_playlist_move_entry
saved_playlist_rename       saved_playlist_delete       saved_playlist_bind_entry
```

`saved_playlist_create` takes its entries with it, which is what keeps creation
atomic for a guest.

The list of saved playlists — id, name, entry count, missing count — is emitted
whole as `saved-playlists` on every change to it, and after anything that can
change a missing count: a bind, a scan, a purge. Entries are read on opening
one. `program:playlist-state` gains the source.

In the renderer `activeTab` is a `ContentType` today and drives the search
query. The _Playlists_ tab is not a content type, so that type widens, and the
track list becomes one of two things the panel can show.

## Open

**Who owns saved playlists in external-library mode.** The tables are in the
library database, and under option B2 of
[external-library.md](./external-library.md) a studio machine's copy of that
database is a replica the next pull overwrites. Either they are the library
owner's, replicated like tracks and edited through it — which makes "build it on
the web page, find it in the studio" need no file at all, at the cost of no
edits while the owner is unreachable — or they are kept out of the replica and
each studio has its own. Using one reads the local copy either way. To be
settled when #505's transport is designed.

## Increments

Each is one pull request.

| #   | increment               | delivers                                                                                                                | needs   | issue             |
| --- | ----------------------- | ----------------------------------------------------------------------------------------------------------------------- | ------- | ----------------- |
| 1   | Saved playlists exist   | tables, commands and their gating, the _Playlists_ tab, the four authoring actions, the missing badge, both appends     | #576    | #577              |
| 2   | Export and import       | the file, binding at import, after a scan and after the analysis pass                                                   | 1       | #501              |
| 3   | Auto-playlist source    | the source in session and `DbRefiller`, the pool predicate, the small-pool note, the empty-pool revert, the switch line | 1       | #503              |
| 4   | Find in library         | binding an unmatched entry by hand                                                                                      | 2       | #580              |
| 5   | Selection and drag-drop | the selection's two saved-playlist actions, selecting entries of a saved playlist, dragging library rows onto one       | 1, #576 | #584, one to file |
| 6   | The web authoring page  | a show built away from the studio                                                                                       | #505    | #581              |

Increment 1 is usable alone: a show built in the app and appended at its hour.
Increment 2 is what #501 asked for, increment 3 what #503 asked for, and
increment 2 completes #577, whose last criterion is an entry becoming playable
when its file arrives.

There is no refactor to land first. In-order play is an append, the source is a
field on a struct the service already builds, and the one engine addition, a
bulk add, arrives with #576.

Increment 4 is the one to drop if it turns out awkward. Without it an unmatched
entry waits for its exact file, and the operator's way round is to remove it and
add the track they meant.

## Not in scope

- **M3U and other players' formats.** Path-based identity; revisit once
  root-relative paths exist (#505).
- **Stop markers in a saved playlist**, and **item overrides** on an entry.
- **In-order as an auto-playlist source.** See
  [Appending to the playlist](#appending-to-the-playlist).
- **A saved playlist's own jingles as the interleave pool.** One switch, when a
  show with its own idents asks for it.
- **Ownership, or a protected flag.** The admin split covers the case they would
  serve.
- **Sync between installs.** That is the open question above.
