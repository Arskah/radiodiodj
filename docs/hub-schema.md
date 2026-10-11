# Reading the hub

A station that shares its library ([shared-library.md](./shared-library.md)) has
it in a Postgres database, and anything that speaks SQL can read it: a web page
that searches the library and builds a show, a report, a script. This page is
the schema as such a reader sees it, and what the app promises about it.

A working example is
[Arskah/music-library](https://github.com/Arskah/music-library): a small web
backend that searches the tracks in the hub, keeps drafts in tables of its own
and hands one out as a saved playlist file. A station is expected to write its
own; that one shows the shape.

## The rules for a reader

- **Read only.** Every row in the `hub_*` tables is written by a RadiodioDJ, in
  a transaction that takes a lock and a revision. A row written by anything else
  is either ignored or applied to every machine's library. Give the reader a
  Postgres role with `SELECT` and nothing more.
- **Its own tables go beside the hub's**, under names that do not start with
  `hub_`, or in a schema of its own.
- **The owner creates the tables**, on its first visit, and creates them empty.
  Until then there is nothing to grant on and nothing to read.
- **What comes back into the app is a file.** A playlist built outside reaches
  the studio as a saved playlist file an admin imports
  ([saved-playlists.md](./saved-playlists.md#the-file)). Its entries are named
  by fingerprint, which is why the reader wants the fingerprint of every track
  it offers.

## The tables

```sql
hub_station (id, library_id text, protocol integer, owner text, settings jsonb)
hub_machines (id text, name text, build text, seen_at timestamptz)
hub_rows (kind text, key text, rev bigint, machine text, edited_at bigint,
          deleted boolean, doc jsonb, waveform bytea, levels bytea,
          PRIMARY KEY (kind, key))
```

`hub_station` has one row. `library_id` names the library the rows belong to,
and changes when the owner's library is replaced: the hub is emptied and filled
again, and every track id in it means something new. `protocol` is the version
of everything on this page.

`hub_machines` is who has connected, and when each last did.

`hub_rows` is the library. A row is one **group** — the smallest thing two
machines can disagree about — named by `kind` and `key`, with its content in
`doc`:

| column      |                                                                             |
| ----------- | --------------------------------------------------------------------------- |
| `rev`       | Taken from one sequence on every write. Higher is later, across all rows.   |
| `machine`   | The id of the machine whose save this is, as in `hub_machines.id`.          |
| `edited_at` | When it was saved, in unix milliseconds by that machine's clock.            |
| `deleted`   | The group is gone. The row stays, so that machines that held it can tell.   |
| `waveform`  | On a `track` only: the drawn waveform, in the app's own format.             |
| `levels`    | On a `track` only: the level table automatic cue points are worked out from |

## The kinds

| kind        | key                    | `doc`                                                                              |
| ----------- | ---------------------- | ---------------------------------------------------------------------------------- |
| `root`      | library path id        | `{"content_type"}` — `music`, `jingle` or `commercial`                             |
| `track`     | track id               | the track, keyed by column name. See [below](#a-track).                            |
| `edit`      | `<track id>:<column>`  | `{"value"}` — a tag an operator changed in the app                                 |
| `cue`       | track id               | `{"fade_in_ms", "fade_out_ms", "trio"}`, `trio` being the three cue points or null |
| `hidden`    | track id               | `{"hidden_at"}` — unix milliseconds, or null when it was shown again               |
| `playlist`  | the list's own id      | `{"name", "created_at", "updated_at", "entries": [...]}`, entries in order         |
| `dismissal` | `<finding kind>:<key>` | `{"value"}` — a library health finding an admin dismissed                          |

A playlist entry is `{"track_id", "fingerprint", "artist", "title", "duration",
"content_type"}`. `track_id` is null for an entry no track answers to.

Nothing about what was played is here: no airing log, no play counts, no
playlist on air.

### A track

The document has one key for each column of the app's `tracks` table
([database.md](./database.md)), under the column's name, except the track id —
which is the row's `key` — the play count, and the columns that travel as an
`edit`, `cue` or `hidden` row of their own. The ones a reader is likely to want:

| key                                                        |                                                                            |
| ---------------------------------------------------------- | -------------------------------------------------------------------------- |
| `fingerprint`                                              | The track's identity outside this library. Null until it has been decoded. |
| `title`, `artist`, `album`, `album_artist`, `genre`        | Text, any of them null.                                                    |
| `year`, `track_no`, `track_total`, `disc_no`, `disc_total` | Integers.                                                                  |
| `initial_key`, `comment`, `isrc`                           | Text.                                                                      |
| `duration`                                                 | Seconds.                                                                   |
| `content_type`                                             | `music`, `jingle` or `commercial`.                                         |
| `missing_since`                                            | Set while the file is not found, null while it is there.                   |
| `root_id`, `path`                                          | The library path the file is under, and where below it.                    |

`path` is relative. Where a library path is on disk is each machine's own
setting and is not in the hub, so nothing here opens a file.

### What a track reads as

Three other kinds change what the app shows for a track, and a reader that
wants the same answer applies them:

- **An edited tag.** Where an `edit` row exists for the track and the column and
  is not `deleted`, its `value` is the tag — null included, which is an
  operator clearing it. A `deleted` one is an edit that was reverted: it still
  carries the value the tag went back to, and that stands while its `rev` is
  higher than the track's.
- **A hidden track.** A `hidden` row whose `hidden_at` is not null takes the
  track out of everything the app lists.
- **A missing track**, with `missing_since` set, is still in the library and
  cannot be played.

A track with a null `fingerprint` cannot be named in a saved playlist file yet.

As a view, for the fields the app's own search covers:

```sql
CREATE FUNCTION tag_of(track hub_rows, col text) RETURNS text
LANGUAGE sql STABLE AS $$
  SELECT CASE WHEN e.key IS NOT NULL AND (NOT e.deleted OR e.rev > track.rev)
              THEN e.doc->>'value' ELSE track.doc->>col END
  FROM (SELECT 1) one
  LEFT JOIN hub_rows e ON e.kind = 'edit' AND e.key = track.key || ':' || col
$$;

CREATE VIEW library AS
SELECT t.key::bigint                       AS track_id,
       t.doc->>'fingerprint'               AS fingerprint,
       tag_of(t, 'title')                 AS title,
       tag_of(t, 'artist')                AS artist,
       tag_of(t, 'album')                 AS album,
       tag_of(t, 'album_artist')          AS album_artist,
       tag_of(t, 'genre')                 AS genre,
       (t.doc->>'duration')::float8        AS duration,
       t.doc->>'content_type'              AS content_type
FROM hub_rows t
LEFT JOIN hub_rows h ON h.kind = 'hidden' AND h.key = t.key AND NOT h.deleted
WHERE t.kind = 'track' AND NOT t.deleted
  AND t.doc->>'fingerprint' IS NOT NULL
  AND t.doc->>'missing_since' IS NULL
  AND h.doc->>'hidden_at' IS NULL;
```

The function and the view are the reader's to create.

## Following changes

A reader that keeps a copy asks for what is new by revision:

```sql
SELECT * FROM hub_rows WHERE rev > $1 ORDER BY rev LIMIT 500;
```

A row is rewritten in place and takes a new `rev` when it changes, so the
highest `rev` seen is the cursor. A purged track arrives as its `track` row with
`deleted` set and no `doc`; its `edit`, `cue` and `hidden` rows are removed
outright, without a revision, so a copy drops them with the track.

When `hub_station.library_id` is not the one the copy was made from, the copy is
of another library: throw it away with its cursor and start from zero.

## What may change

- **A key in a document is a column name.** A column added to the app adds a
  key. A reader ignores the ones it does not know and does not rely on one this
  page does not name.
- **`hub_station.protocol`** goes up when a build changes what the rows mean:
  a new kind, a key that changes sense. A reader written for one protocol
  should refuse a hub on another rather than guess. This page describes
  protocol 2.
- **The hub is not a backup.** The owner empties it when its own library is
  replaced, and can fill it again from nothing.
