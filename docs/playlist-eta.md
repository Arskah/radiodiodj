# Playlist ETA and planned Stops

**Implementation spec.** Planned Stop durations, held Stops and stable Upcoming
ETAs, layered above prepared shows and playlist blocks.

The starting base is `ebc1ed0`. Keep this as a third change; if the earlier PRs
are squash-merged, transplant only this branch's new commits onto `main`.

## Planned duration

Every Stop marker can have an optional planned duration: in a saved prepared
show, a queued block or the top-level Playlist. Ordinary saved playlists still
cannot contain Stops.

Use an editable `m:ss` field on the Stop row. Blank means unspecified; `0:00`
is an explicit zero-length plan. Accept non-negative whole seconds, including
minutes above 59. Reject invalid input without changing the previous value.
Existing saved-list permissions apply. Queued edits affect that copy only.

A duration estimates the presenter's pause. It never starts playback, removes
a Stop or changes Auto/Manual mode. Reaching zero never resumes playback.

## Holding at a Stop

Consuming a Stop records **Holding at stop** separately from a cleared deck,
an exhausted Playlist or a paused track. Keep its planned duration, elapsed
hold time and block association after removing its row from Upcoming.

The main deck shows the row's existing `block` icon, **Stop marker** as its
title and **Holding at stop** as its status. Where it normally shows remaining
time, it shows:

- Planned Stop: elapsed minus planned duration, starting at `−m:ss`, passing
  through `0:00`, then counting upward with a `+` sign. After the deadline, a
  steady **Over plan** label and warning colour indicate the overrun; no flash.
- Unplanned Stop: elapsed time from `0:00` upward, without an overrun signal.

The consumed Stop is read-only. No duration editor on the deck. Hide its audio
waveform and disable seeking and track-only controls; Play and Next retain
their existing transport meaning. Leaving the hold for another item clears it;
reaching another Stop starts a fresh hold. Pressing Stop clears it to the
ordinary stopped state.
Adding, removing or reordering Upcoming does not end the hold. Its block header
remains active while holding, including at a trailing Stop.

Restore a held Stop at its last saved elapsed time and continue counting from
reopening. Time with the app closed does not count: `−1:07` can still be
`−1:07` after reopening half an hour later. No special recovery logic is needed.
Never restore into audio playback. Project ETAs afresh rather than saving them.

## Clock and ETA display

Show the host computer's local `HH:MM:SS` beside the toolbar's station brand,
including when the brand is an image. Upcoming rows show the expected start
time to the left of their existing duration. Stop rows can show when their
hold is expected to begin. Unknown ETAs display `—`.

Blocks contribute no time. Their children use the same continuous schedule as
standalone items; collapsing a block changes nothing. History and library
search do not acquire ETAs. Saved shows retain planned durations, not start
times.

| Playback state                      | Upcoming ETAs                                                  |
| ----------------------------------- | -------------------------------------------------------------- |
| Playing                             | Fixed until the sequence, timing or transport actually changes |
| Paused track                        | Shift together; include the track's remaining time             |
| Stopped or empty deck with Upcoming | Shift together, first playable item estimated to start now     |
| Holding within a planned Stop       | Fixed at the planned resumption time                           |
| Holding beyond its plan             | Shift together, first next item estimated to start now         |
| Holding at an unplanned Stop        | Shift together from now                                        |

An unplanned Stop still ahead is a barrier: its own arrival can be estimated,
but all later ETAs are unknown. Once it is reached, following ETAs become
available and move together, up to the next unknown barrier. Planned Stops
add their duration and allow projection beyond them. These are estimates of
the presenter resuming on time, never promises of automatic playback.

Manual mode also shows estimates, assuming the presenter starts each next
track immediately at Cue Out. While the presenter waits, estimates slip.
Auto mode uses Next Start for a track-to-track transition, and Cue Out before
a Stop. Resolve station cues and existing item overrides through the backend's
current cue rules. Account for the two-deck limit: an audible tail can delay
the next overlap; never project three simultaneous tracks.

Existing unattended-air-time totals still end at the first Stop. Planned
pauses do not count as audio coverage.

Known unavailable tracks are skipped as playback skips them, never skipping a
Stop. A track whose required timing is unknown makes subsequent ETAs unknown
until that timing becomes known or it is removed/skipped.

## Stable timing

The previous ETA branch projected `now + duration − last received position` in
the renderer. Its clock refreshed once a second; position updates could
recalculate against that stale clock between refreshes, then jump forward at
the next refresh. Independent event arrival also affected rounding. Do not
reuse that timing model.

Keep one schedule in the backend, keyed by queue occurrence ids. A small pure
timing module accepts the ordered items and coherent transport observations;
the Playlist module retains ownership of playback and Stop consumption.

1. Capture position, resolved cues, playback state and time together in the
   main-deck worker at meaningful transitions: actual audio start, pause,
   resume, seek, handover, end and buffering changes. Include the audible tail's
   remaining time when it constrains the next handover. Include a sequence and
   playback-instance identity so delayed observations and repeated track ids
   cannot associate timing with the wrong airing. An early “playing” report
   while loading is not an audio start.
2. Establish one schedule origin. Keep fractional timing when accumulating
   track spans, then truncate each absolute ETA to a whole second once. Never
   round individual track durations before summing them.
3. Ordinary position events update the playhead only. Automatic consumption,
   successful handover and unchanged snapshots retain the existing schedule;
   do not reset its origin from every incoming track's actual start time.
   Event delivery latency and normal worker-tick timing must not move labels.
4. A waiting interval advances one shared delay. Render every known ETA as
   `baseSecond + sharedDelaySeconds`, using the same delay for every row.
   Preserve fractional accumulated delay internally across pauses; round the
   shared delay once. A planned hold adds delay only after its deadline.
5. Resume freezes that delay without changing the displayed labels. Queue
   edits, seeks, early skips, changed cue/duration data and actual loading or
   output interruptions can revise the affected schedule. Changes that only
   affect a suffix must not disturb earlier ETAs. Real delays are corrected
   together, never by independently smoothing or clamping each row.

The renderer receives the schedule with the Playlist snapshot and uses one
shared clock for the toolbar, Stop counter and moving delay. Timer callbacks
sample elapsed time; they do not increment independent per-row counters. A
late callback catches up every row in one render.

Use monotonic elapsed time for holds and delay, with a host wall-clock anchor
for displayed start times. The transport sample supplies its capture time;
receiving it is not a new start time. A deliberate host clock correction may
re-anchor wall times once, as one schedule change. Routine progress cannot.

This needs a small, coherent timing observation from the audio worker, not a
playback rewrite. Keep existing time events for waveforms. Extrapolate display
time locally; do not run refill, database work or Playlist transitions each
second. Keep schedule and queue revisions together so an old snapshot cannot
replace newer timing.

## Storage

Add nullable `planned_duration_seconds` to saved entries in one appended
migration after prepared shows (currently step 20). It is valid only for Stops;
do not reuse the existing audio `duration` column. Old Stops default to null.
Add the migration seed and regenerate the schema snapshot as usual.

Carry `plannedDurationSeconds` through saved entries, runtime items, copied
blocks, Save as, sessions, files and whole-list synchronization. Only the plan
travels with saved lists; active holds, elapsed time and ETAs stay local.

Advance the saved-file version (currently 2) and hub protocol (currently 3),
retaining imports of supported older files. An older reader must refuse the
new format rather than silently discard the plan on a save or sync round trip.
Keep existing prepared-show migrations unchanged.

## Acceptance

- A planned Stop survives saving, queueing, Save as, file transfer, sync and
  restart. Changing its queued copy leaves the saved original intact.
- A two-minute Stop displays `−2:00`, reaches zero, then shows overtime without
  resuming. An unplanned Stop only counts elapsed time. Neither is editable
  after consumption. Stop-button and empty-deck states never impersonate it.
- With fractional track lengths and deliberately delayed/reordered progress
  events, every known ETA stays unchanged throughout uninterrupted playback,
  including normal handovers and snapshot refreshes.
- Pausing for several seconds shifts all known ETAs by the same amount.
  Resume freezes them without a jump. Repeat pauses to catch fractional drift.
- A planned Stop keeps later ETAs fixed until its deadline, then shifts them
  together. An upcoming unplanned Stop hides later ETAs; reaching it reveals
  the next stretch and starts its shared movement.
- Test Next Start versus Cue Out, a short track under an audible tail, Manual
  mode, seeks, skipped/unavailable tracks, loading delays, repeated tracks,
  queue edits, collapsed blocks and midnight. Use an injected clock, not sleeps.

Automatic fallback and changes to commercial scheduling remain separate work.
