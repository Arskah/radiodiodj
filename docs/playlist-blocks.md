# Playlist blocks

**Specification. Not implemented.** Issue [#683](https://github.com/Arskah/radiodiodj/issues/683).
Builds on [Prepared shows](./prepared-shows.md) (#682).

A whole saved playlist enters the on-air playlist as one named, expandable
block. Tracks taken individually from a saved playlist enter as ordinary
tracks. Both playlists and prepared shows use this block UI.

## Adding material

| Action                                 | Result in Upcoming                                                                                  |
| -------------------------------------- | --------------------------------------------------------------------------------------------------- |
| Drag a whole saved playlist            | One block at the indicated position                                                                 |
| Choose **Add playlist**                | The same block, appended                                                                            |
| Drag individual tracks or a selection  | Ordinary tracks, with no new block                                                                  |
| Press **+** or choose **Add N tracks** | Ordinary tracks appended outside every block                                                        |
| Choose **Add as next**                 | First remaining children of the active block, or the front of the main playlist when none is active |

The whole-playlist drag starts from its own handle on the saved-playlist row
or the title of an open saved playlist. Dragging entry rows always means those
entries, even when every track is selected. A one-track playlist still makes a
block when added whole.

Whole-playlist dragging and **Add playlist** use stored order, regardless of
search or sort. They never generate extra material. The existing explicit
**Add with jingles and commercials** action for ordinary playlists queues its
resulting sequence as a block. Prepared shows use their authored sequence.

**+** and ordinary add actions append after everything in the main playlist,
outside every block. **Add as next** inserts before the active block's
remaining children, including any Stop. Several tracks land as one contiguous
run in selection order. It also works while the block is collapsed or its last
track is on air. Adding never starts or resumes playback.

## The block in Upcoming

The header is about the size of a track row: a disclosure button, a playlist
icon, its title and the number of remaining tracks. Expanded, its remaining
entries appear immediately below, indented. Stop markers have the same
indentation as tracks. Newly added blocks start expanded.

```text
Standalone track
▾ [playlist] John's morning show · 3 tracks
    Opening jingle
    Track A
    Stop
    Track B
Standalone track
```

Collapsing hides the children and keeps the header. The labelled disclosure
button is keyboard-accessible and exposes its expanded state. Collapse changes
presentation only, never playback or order.

Whole blocks are placed between top-level entries. Blocks do not nest. A drop
line shows the destination: full-width between top-level entries, indented
between the children of an expanded block. Dropping tracks into the indented
gap inserts them at that position within the queued block.

Dropping tracks onto a collapsed block's header appends them as its last
children without expanding it. Highlight the header to show an inside drop;
a line before or after the block still means outside it.

Tracks copied from the library or a saved playlist, and tracks moved from
elsewhere in the on-air playlist, use these same destinations. Moving a queued
track removes it from its previous position. Neither path creates another
block or changes the saved original.

## As the block airs

When a child becomes Now playing, it leaves the remaining children and appears
on the main deck. The header stays at the top of Upcoming and remains visible
while its children scroll. Collapsing an active block keeps its header and
playing status visible.

The header stays while the last child is on the main deck, even with no tracks
remaining underneath it. It disappears when the block has no remaining entries
and its last track leaves Now playing. Handover ends that association; the
outgoing track's audible tail does not keep the header.

Stops follow the prepared-show rules. An unfinished block stays active and
visible while waiting at a Stop; paused playback also retains it. **Add as
next** can change what will play after the presenter resumes, but never resumes
it. A trailing Stop still halts before the next top-level entry.

Collapse state does not affect advancement, handover, prefetch or unavailable
tracks. Skipping unavailable audio must never skip over a Stop.

## The saved original and the queued copy

- Each insertion copies the title, kind and ordered entries into a new block.
  Editing, renaming or deleting the saved original does not change a queued
  copy. Queuing the same playlist twice makes independent blocks.
- Remaining children can be reordered or removed locally. An unstarted block
  can be moved as a unit. The active header stays pinned.
- Removing a block removes its remaining entries without cutting the track
  on air. Its header remains until that track leaves the main deck.
- **Save as** flattens the remaining blocks and standalone entries into one
  saved list, preserving Stops and excluding Now playing. The kind selection
  and station cue-point rules are those of prepared shows.

## Queue state

The backend owns membership and advancement. Whole snapshots carry the block
structure and current block association. Commands identify the particular
block and child occurrence: track ids cannot distinguish repeated entries,
and visible row indices change when blocks are collapsed. Keep one playback
engine for standalone entries and block children.

Blocks, remaining children and the current track's block association survive
restart. Read old flat sessions; restore paused as today. The saved original
is never consumed.

Prefetch and air-time calculations traverse children in playback order;
headers contribute no audio or duration. The existing total until the first
Stop still ends there, including in a collapsed block. Refill counts remaining
contents rather than treating a whole show as one track, and never inserts
material inside a queued block.

## Acceptance scenarios

1. Adding a whole saved playlist makes a block in stored order. Selecting and
   queueing all its tracks makes ordinary tracks instead.
2. During a block, **+** appends a standalone track after everything queued.
   With `B, Stop, C` remaining, **Add as next** for `X, Y` produces children
   `X, Y, B, Stop, C`. With no active block, it prepends to the main playlist.
3. Dropping onto a collapsed block appends inside it and leaves it collapsed.
   Moving a queued track removes its former entry. A drop before or after the
   block keeps it outside.
4. Children disappear as they become Now playing. The header remains through
   the last track; adding as next during that track extends the block.
5. A Stop in a collapsed block still halts at Cue Out. Later children remain
   queued, and a trailing Stop prevents advancing to the next top-level entry.
6. Queue edits leave the saved original and other copies alone. Repeated
   tracks and playlists are targeted by occurrence, including after restart.
   Collapse changes presentation only; adding never starts playback.

Automatic fallback and its shadow row are deferred, along with dynamic
commercial breaks and remote cue suggestions. An explicit Stop remains an
intentional pause.
