# RadiodioDJ

Desktop radio-station player: scans local audio, classifies tracks by content type, plays them on virtual DJ decks with auto-playlist scheduling that interleaves jingles and commercials between music.

## Language

### Library & content

**Track**:
A single audio file indexed in a library, with tag-derived metadata.
_Avoid_: Song, file, audio, item

**Content type**:
Closed enum classifying a track: `music`, `jingle`, or `commercial`. Determines which typed library owns it.
_Avoid_: Category, kind, tag

**Music library**:
Aggregate of all tracks with content type `music`.
_Avoid_: Songs, music collection

**Jingle library**:
Aggregate of all tracks with content type `jingle`.
_Avoid_: Sweepers, IDs, stings

**Commercial library**:
Aggregate of all tracks with content type `commercial`.
_Avoid_: Ads, spots

**Library path**:
A user-configured filesystem root the scanner recurses into. Feeds tracks into one or more typed libraries.
_Avoid_: Folder, source, watch dir

> "Library" alone is ambiguous — always qualify with the content type.

### Scan lifecycle

**Scan**:
Traversal of all library paths that upserts present tracks, reattaches moved ones and prunes the rest.
_Avoid_: Index, crawl, refresh

**Prune**:
Marking tracks **Missing** when their file is gone from a fully listed library path, or no library path covers it any more. Never deletes. A library path that cannot be read prunes nothing.
_Avoid_: Cleanup, gc, sweep, delete

**Missing track**:
A track whose file a scan could not find. Hidden from every library but kept with its id, cue points and play count until **Purge**.
_Avoid_: Deleted, orphan, stale

**Fingerprint**:
Hash of a track's audio (codec parameters plus the first MiB of packet data), independent of path and tags. Identifies the same recording at a new path.
_Avoid_: Checksum, file hash

**Reattach**:
Matching a file at a new path to the **Missing track** with the same **Fingerprint**, so the track keeps its id and everything on it.
_Avoid_: Relink, merge, re-import

**Purge**:
The operator's explicit, permanent deletion of **Missing tracks**. The only way a track row is deleted.
_Avoid_: Prune, cleanup

**Duplicate**:
A present Track sharing its **Fingerprint** with another present Track. Starts as a copy of the original's operator state and is its own Track from then on. Removed only by the operator deleting its file.
_Avoid_: Copy, clone, twin

**Possible duplicate**:
Present music Tracks with the same normalised artist and title but no shared **Fingerprint**. A notice for the operator, not a match.
_Avoid_: Near-duplicate, fuzzy match

**Library check**:
A listing of every **Library path** compared with the library by path and mtime alone, reporting new, changed and gone files and unreachable paths. Reads no tags or audio and never changes the library.
_Avoid_: Quick scan, watch, sync

**Disk change**:
A difference a **Library check** found that no **Scan** has applied yet.
_Avoid_: Pending change, unscanned file

**Library health**:
The report of **Missing tracks**, **Duplicates**, **Possible duplicates** and **Disk changes**, and the Settings view that shows it.
_Avoid_: Library status, diagnostics

**Dismissal**:
The operator's acknowledgement of one health finding. Silences the badge only while the finding stays exactly as it was dismissed.
_Avoid_: Ignore, mute, snooze

**Delta cache**:
mtime + content-type cache letting the scanner skip unchanged files.
_Avoid_: Cache, diff

**Scan progress**:
Event stream emitted by the scan worker with counts and current file.
_Avoid_: Status, update

### Decks & playback

**Deck**:
Independent playback channel; loads one track, plays, pauses, seeks. Modeled on a real-DJ rig.
_Avoid_: Player, channel, engine

**Program bus**:
The mixer summing every on-air Deck into the main output device. One output stream, one sink per deck.
_Avoid_: Master, output, PGM

**Deck role**:
What a Deck is doing right now: `main` (on air, defines Now playing), `arm` (loaded with the next Playlist item, awaiting Handover) or `tail` (handed over, playing out the outgoing track's Overlap). Roles move between decks; decks do not move between roles.
_Avoid_: A deck, B deck, slot

**Main deck**:
The Deck currently holding the `main` role. Its output is what listeners hear. A role, not a fixed deck.
_Avoid_: Program deck, A deck

**Tail deck**:
The Deck holding the `tail` role: the outgoing track after Handover, still audible on the Program bus but no longer Now playing. Vacated at its Cue out, at which point it becomes the arm Deck.
_Avoid_: Outgoing deck, old deck, B deck

**Overlap**:
The span between Handover and the outgoing track's Cue out, during which the Main deck and the Tail deck are both audible.
_Avoid_: Crossfade, blend

**Cue deck**:
Off-air deck on a separate output device, used to audition a track and its Cue points. Never on the Program bus.
_Avoid_: Preview, monitor, B deck

**Cue editor**:
The dialog where Cue points are placed, over a waveform with draggable handles. Auditions through the Cue deck, and is the only surface that moves a marker.
_Avoid_: Cue dialog, marker editor, trim editor

**Handover**:
The moment the arm Deck begins playing and takes the `main` role, triggered by the outgoing track's Next start cue point.
_Avoid_: Crossfade, transition, segue

**Now playing**:
The track currently loaded and playing on the Main deck.
_Avoid_: Current, active

**Cueing**:
Loading and inspecting a track on the cue deck without putting it on air.
_Avoid_: Previewing, scrubbing

**Seek**:
Reposition playback within the track loaded on a deck.
_Avoid_: Scrub, skip

### Track shaping

**Cue point**:
A position in a Track marking where playback starts, reaches full volume, begins fading, hands over, or stops. Stored per track; never modifies the audio file.
_Avoid_: Edit, marker, trim

**Cue in / Fade in / Fade out / Cue out / Next start**:
The five Cue points. Start of audio; point where full volume is reached; point where the ramp down begins; point where playback stops; point where the next track begins.
_Avoid_: In point, out point, ramp

**Radio edit**:
The set of Cue points stored on a Track. The default for every airing of it.
_Avoid_: Preset, default edit

**Item override**:
Cue points carried by a single Playlist item, overriding that track's Radio edit for one airing only. Never written back to the Track.
_Avoid_: Temp edit, local edit

**Air time**:
`cueOut - cueIn` — the duration that actually reaches air. Distinct from the Track's file duration.
_Avoid_: Effective duration, real length

**Air timeline**:
Playback position measured from Cue in, so `0` is the first audible sample. All deck IPC speaks this timeline; only the player worker knows source-absolute positions.
_Avoid_: Edited time, local time

### Playlist

**Playlist**:
Ordered sequence of upcoming tracks that feeds the Main deck.
_Avoid_: Queue, list

**Auto-playlist**:
Playlist mode that maintains itself by randomly selecting from the Music library with jingle/commercial interleaving.
_Avoid_: Auto-DJ, autoplay

**Lookahead buffer**:
The buffer of tracks the auto-playlist keeps ahead of Now playing. Target size is 20 tracks; refills when remaining tracks fall below a threshold of 5.
_Avoid_: Buffer, preload

**Interleave**:
Insertion of one Jingle library track every 4 music tracks and one Commercial library track every 8.
_Avoid_: Rotation, scheduling

### Appearance

**Theme**:
A named set of colour values the operator switches between: a built-in, or a
folder dropped into `{app_data_dir}/themes/`. Repaints the whole player. Changes
no layout, no wording and no behaviour.
_Avoid_: Skin, style, stylesheet, mode, dark mode

**Theme token**:
One named colour in the **Theme** contract (`--surface`, `--cue-in-color`,
`--scrim`), and the only kind of thing a **Theme** may set. Spacing, radius and
font custom properties are deliberately not theme tokens.
_Avoid_: Variable, CSS var, custom property, colour

**Base**:
Whether a **Theme** paints on light or dark. Declared by the theme, and used for
two things: filling in the **Theme tokens** it left out, and telling the OS which
chrome to draw for scrollbars, selects and form controls.
_Avoid_: Mode, dark mode, appearance, scheme

**Station identity**:
The operator's own name and artwork for their station — a station name, a
**Toolbar logo** and a **Record label**. Configured separately from the **Theme**
and unaffected by it.
_Avoid_: Branding, theming, white-label, customisation

**Toolbar logo**:
The **Station identity** image drawn in the toolbar in place of the station's
name. Wide-friendly; scaled to the toolbar's height.
_Avoid_: Brand, icon, app icon, wordmark

**Record label**:
The **Station identity** image drawn at the centre of a deck's vinyl when the
track has no cover art. Square, and cropped to a circle.
_Avoid_: Logo, artwork, cover, sleeve

### Persistence

**Config**:
Persisted `AppConfig` written to `{app_data_dir}/config.json`.
_Avoid_: Settings, prefs

**Session**:
Persisted `SessionState` written to `{app_data_dir}/session.json`.
_Avoid_: Restore state

**Flush save**:
Awaited write of session/config on window close.
_Avoid_: Persist, sync

## Relationships

- A **Library path** contributes **Tracks** to one or more typed libraries, selected by **Content type**
- Every **Track** belongs to exactly one of **Music library**, **Jingle library**, or **Commercial library**
- A **Playlist** feeds the **Main deck**; the **Cue deck** is fed by manual selection from any library
- An **Auto-playlist** draws music from the **Music library** and **Interleaves** jingles and commercials from their respective libraries
- An **Auto-playlist** keeps a **Lookahead buffer** ahead of **Now playing**
- Only music tracks advance the **Interleave** counters
- Every on-air **Deck** feeds the **Program bus**; the **Cue deck** does not
- Exactly one **Deck** holds the `main` **Deck role** at a time; **Handover** moves it
- Two program **Decks** means no **Deck** is armed during an **Overlap**: the **Tail deck** becomes the arm **Deck** when it is vacated, and only then is the following **Playlist** item loaded
- A **Track** may carry a **Radio edit**; a **Playlist** item may carry an **Item override** that wins for that airing
- **Air time** derives from the **Cue points** that apply to an airing, not from the **Track**'s file duration
- A **Library check** predicts what the next **Scan** would do; only the **Scan** applies it, and only **Purge** deletes
- A **Dismissal** silences a **Library health** finding without hiding it
- A **Track** is identified by its row, not its path: **Prune** makes it **Missing**, **Reattach** or a returning path restores it, and only **Purge** deletes it

- A **Theme** sets some or all **Theme tokens**; the rest come from the built-in matching its **Base**
- Exactly one **Theme** is active; `appearance.themeId` in **Config** names it
- A **Theme** that fails validation is listed and cannot be selected; it is never partly applied
- A **Theme** may ship a **Toolbar logo** and a **Record label**; the operator's **Station identity** wins over both
- Cover art wins over the **Record label**, which wins over the shipped label
- A **Theme** never sets the station name

## Example dialogue

> **Dev:** "When I drop a folder of station IDs into a **Library path**, does the **Auto-playlist** start using them?"
>
> **Domain expert:** "Only if they're tagged with content type `jingle`. The **Scan** sorts them into the **Jingle library**. The **Auto-playlist** then picks one every 4 music tracks via **Interleave**."
>
> **Dev:** "What if I want to preview a specific commercial before it airs?"
>
> **Domain expert:** "Load it on the **Cue deck**. **Cueing** lets you audition without affecting **Now playing** on the **Main deck**."
>
> **Dev:** "Can the **Cue deck** play tracks from the **Music library** too?"
>
> **Domain expert:** "Yes. A deck is content-type-agnostic. The library distinction only matters for **Interleave** selection in the **Auto-playlist**."
>
> **Dev:** "This song has eight seconds of intro. Do I have to edit the file?"
>
> **Domain expert:** "No — set its **Cue in** in the **Cue editor** and save it to the track. That is a **Cue point**, stored on the **Track** as its **Radio edit**, and it applies to every airing from then on. The file is never touched."
>
> **Dev:** "What if I want it shortened just this once, for tonight's show?"
>
> **Domain expert:** "Same editor, but press _Use once_ instead of saving — the **Playlist** item carries an **Item override**. That wins for that one airing and never writes back to the **Track**."
>
> **Dev:** "The library says 5:02 but the playlist says 3:34. Which is right?"
>
> **Domain expert:** "Both — 3:34 is the **Air time**, what actually reaches air once **Cue in** and **Cue out** apply. Every duration in the operator UI means air time; the tooltip shows the file duration too."
>
> **Dev:** "When does the next song actually start?"
>
> **Domain expert:** "At the outgoing track's **Next start** cue point. That triggers **Handover** — the arm **Deck** starts playing and takes the `main` **Deck role**, while the outgoing one plays on toward its **Cue out**. Both are summed on the **Program bus** meanwhile."
>
> **Dev:** "I dropped `station-red` into the themes folder and it isn't in the list."
>
> **Domain expert:** "It is — press _Reload themes_. If it shows greyed, the row says why: probably a **Theme token** name that isn't in the contract. A **Theme** with a bad token is refused whole, never half-applied, so you never end up with two palettes on screen at once."
>
> **Dev:** "If I switch to Daylight, do I lose my logo?"
>
> **Domain expert:** "No. The logo is **Station identity**, not part of the **Theme**. A theme can ship a default one, but yours wins over it. Only the colours change."
>
> **Dev:** "If a **Library path** is removed, what happens to **Now playing** if it points to a track from there?"
>
> **Domain expert:** "Playback continues — the **Main deck** holds the decoded source. After the next **Scan**, **Prune** marks the track **Missing**, so no **Auto-playlist** refill picks it. Add the path back and the next **Scan** brings it back with its cue points intact."

## Flagged ambiguities

- "Library" alone is ambiguous → always qualify: **Music library** / **Jingle library** / **Commercial library**. The plain word survives only as a generic shorthand.
- "Library path" is filesystem input, not a library. Many-to-many with typed libraries (one path can feed all three; one library aggregates many paths).
- "Player" retired as a domain term → use **Deck**. "Player" remains an implementation detail (Rust worker driving a rodio Sink per deck).
- "Playlist" vs "Queue" → **Playlist** is canonical. Avoid "queue" to prevent confusion with **Lookahead buffer**.
- "Auto-playlist" is a mode of **Playlist**, not a separate concept.
- "Cue deck" is a Deck (not a UI label), modeled on real-DJ rigs. It is **not** a peer of **Main deck**: Main deck is a **Deck role** that moves between decks on the **Program bus**, while the Cue deck is a fixed off-air deck on its own output device.
- "Cue point" is a position in a Track; "Cue deck" is the off-air deck; the "Cue editor" is where points are placed. The overlap is inherited from playout software convention.
- Bare "edit" means **metadata/tag editing** and nothing else. The playback markers are **Cue points**; the stored set of them is a **Radio edit**. Tag-editing code says `metadata` explicitly for this reason.
- "Segue" and "crossfade" are not domain terms → use **Handover**, which is triggered by a Cue point rather than a configured duration.
- "Check" vs "Scan" → a **Library check** only reads the disk and reports; a **Scan** changes the library. Never call a check a "quick scan".
- "Ignore" is not a domain term → a **Dismissal** silences a finding, and there is no ignored-track state. An unwanted **Duplicate** is removed by deleting its file.
- "Content type" is a closed enum: `music | jingle | commercial`. New types require deliberate domain extension.
- "Theme" is colours only. A change that needs different spacing, fonts or wording is a redesign, not a theme.
- "Logo" alone is ambiguous → **Toolbar logo** and **Record label** are two images with different shapes and different fallbacks. The **app icon** is neither, and no theme touches it.
- "Dark mode" is not a domain term → a **Theme** has a **Base**, and the operator picks a theme, not a mode. The app never follows the OS.
- "Palette" is informal for the values inside a **Theme**; the term of art is **Theme token**.
- "Branding" retired in favour of **Station identity**, so the config fields, the commands and the Settings heading all say the same word.
