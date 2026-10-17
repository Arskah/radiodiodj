# Prepared shows

**Specification. Not implemented.** Issue [#682](https://github.com/Arskah/radiodiodj/issues/682).

Saved playlists gain a `playlist | prepared_show` kind, shown as **Playlist**
or **Prepared show** in one selector. Both use the same feature, explicit entry
order and track controls. Both can contain music, jingles and commercials;
only a prepared show can contain Stop markers.

A presenter can prepare the sequence before a show, including deliberate
pauses, and add it to the on-air playlist when needed. This increment keeps the
current flat on-air playlist and its existing Stop behavior.

## Authoring

The editor offers **+ Stop** for prepared shows. It appends a marker that can
be dragged into place and removed like a track. Changing a prepared show back
to a playlist is refused while it contains Stops; remove them first. Enforce
this in the backend as well as the UI. Existing saved-playlist permissions
apply to changing the kind and entries.

Ordering changes use the stored order view. Sorting and searching remain
browsing tools and never change the saved sequence. Repeated tracks are allowed.

**Save as** keeps the remaining queued tracks and Stops, excluding Now playing.
Its kind selector defaults to **Prepared show** when those entries contain
Stops, and **Playlist** otherwise. Stops cannot be saved under **Playlist**.

Saved entries reference the station's cue points. They contain no cue-point
overrides. Any change to the current **Use once** action is separate work.

## Queueing a show

Adding a show appends its tracks and Stops in stored order, in one transition.
It never starts or resumes playback. The saved original stays intact as the
queued copy is consumed.

A Stop lets the preceding track finish at Cue Out, then waits for the
presenter. Later entries stay queued. A trailing Stop halts before anything
queued after the show. There is no implicit Stop at the end of a show.
Skipping unavailable audio must never skip over a Stop.

The existing **Add with jingles and commercials** action stays for ordinary
playlists. Prepared shows use their authored sequence, with deliberately
placed commercials and jingles. Dynamic commercial breaks are later work.

## Storage and transfer

The database already stores `saved_playlist_entries.position` and reads entries
by `position, id`. Keep that order. Existing lists become `playlist`, and all
existing entries, including unmatched ones, become track entries.

A saved entry is a track or a Stop. A Stop has no audio identity and is never
represented as an unmatched track. Missing counts and fingerprint binding must
exclude Stops. Show track and Stop counts distinctly; a Stop has no duration.
Choosing either kind as the auto-playlist source still draws only from its
music tracks.

The kind and ordered entries survive saving, import/export and shared-library
synchronization. Existing track-only files import as `playlist`. Use a new file
version that older apps refuse, rather than letting a Stop become an unmatched
track and disappear. Refuse a file claiming to be `playlist` while holding
Stops. Shared-library compatibility needs the same protection.

Changes to the entry schema follow the existing appended-migration and schema
snapshot rules. Update the saved-playlist synchronization document alongside it;
the existing later-save-wins rule still applies to the whole saved list.

## Acceptance scenarios

1. Existing lists and track-only files become `playlist` with their order
   intact. Both kinds share the saved-playlist UI and track controls.
2. Only `prepared_show` accepts Stops. Changing a show with Stops to `playlist`
   fails without changing it. An unmatched track stays distinct from a Stop.
3. `Track A, Stop, Track B` survives storage, Save as, a file round trip and
   synchronization in that order. Its Stop remains effective after queueing
   and restart, even if an adjacent track is unavailable.
4. Queueing and playing a show leaves the saved original intact. A trailing
   Stop prevents advancing to later queued material. Queueing alone never
   starts playback.
5. Ordinary playlists retain **Add with jingles and commercials**. Prepared
   shows queue their authored sequence and use the station's cue points.

Playlist grouping, automatic fallback, dynamic breaks and remote cue
suggestions are separate work. The remote creator can keep exporting existing
track-only files; letting it author Stops belongs in its own repository.
