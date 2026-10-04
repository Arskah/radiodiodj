const DOCS = "https://github.com/Arskah/radiodiodj/blob/main/docs";

/** One selling point, with the design doc behind it when there is one. */
export interface Feature {
  title: string;
  body: string;
  doc?: string;
}

/** A titled set of features, in page order. */
export interface FeatureGroup {
  title: string;
  features: Feature[];
}

export const featureGroups: FeatureGroup[] = [
  {
    title: "On air",
    features: [
      {
        title: "Two-deck program bus",
        body: "Decks are summed into one output, so two tracks can be audible at once. A transition is a handover at the outgoing track's Next start marker, not a fixed crossfade.",
        doc: `${DOCS}/program-bus.md`,
      },
      {
        title: "Cue deck",
        body: "Audition any track on a second output device without touching what is on air, then add it to the playlist.",
        doc: `${DOCS}/audio.md#the-cue-deck`,
      },
      {
        title: "Auto or manual",
        body: "AUTO advances through the playlist, MANUAL stops after each track, and stop markers park the show at a fixed point in the queue.",
      },
      {
        title: "ReplayGain levelling",
        body: "Every track is measured against one reference, so a 1970s master and a modern one sit at the same level.",
        doc: `${DOCS}/audio.md#replaygain`,
      },
    ],
  },
  {
    title: "Cue points",
    features: [
      {
        title: "Five markers per track",
        body: "Cue in, fade in, fade out, cue out and next start, stored in the library and applied at every airing. The audio file is never modified.",
        doc: `${DOCS}/cue-points.md`,
      },
      {
        title: "Cue editor",
        body: "A two-strip waveform editor with draggable handles, keyboard nudging, pre-roll and live audition through the cue deck.",
        doc: `${DOCS}/cue-points.md#authoring`,
      },
      {
        title: "Automatic cue points",
        body: "Silence-trimmed starts and ends and a music segue point, derived during analysis. An unprepared library airs tight without anyone touching a marker.",
        doc: `${DOCS}/cue-auto-analysis.md`,
      },
    ],
  },
  {
    title: "Library",
    features: [
      {
        title: "Three content libraries",
        body: "Separate folders and browsing for music, commercials and jingles, with full-text search across the tags.",
        doc: `${DOCS}/library.md`,
      },
      {
        title: "Stable track identity",
        body: "Moving, renaming or re-adding files keeps each track's cue points, play count and edits.",
        doc: `${DOCS}/track-identity.md`,
      },
      {
        title: "Network-share resilience",
        body: "Tracks are read whole into RAM and prefetched ahead of the playlist, so a share that stalls mid-show does not stall the output.",
        doc: `${DOCS}/audio.md#whole-file-reads`,
      },
      {
        title: "Library health",
        body: "Missing tracks, duplicates, unreadable files and disk changes in one report.",
        doc: `${DOCS}/library-health.md`,
      },
    ],
  },
  {
    title: "Programming",
    features: [
      {
        title: "Auto playlist",
        body: "Continuous playback that keeps a lookahead buffer queued and refills itself from the music library.",
        doc: `${DOCS}/playlist.md`,
      },
      {
        title: "Interleave",
        body: "A jingle every 4 music tracks and a commercial every 8, both configurable.",
      },
      {
        title: "Rotation rules",
        body: "Never reselect a track, or an artist, that aired inside a configurable window, backed by a persistent airing log.",
        doc: `${DOCS}/rotation.md`,
      },
    ],
  },
  {
    title: "Station",
    features: [
      {
        title: "Now-playing broadcast",
        body: "An outbound webhook plus atomic file output for stream overlays, metadata bridges and scripted consumers.",
        doc: `${DOCS}/now-playing-broadcast.md`,
      },
      {
        title: "Themes and identity",
        body: "Colour schemes as a folder you can write yourself, plus your station name, toolbar logo and record label on the deck vinyl.",
        doc: `${DOCS}/theming.md`,
      },
      {
        title: "Admin mode",
        body: "A password that locks settings and destructive actions while leaving playback, cueing and browsing open to whoever is on shift.",
        doc: `${DOCS}/admin-mode.md`,
      },
    ],
  },
];
