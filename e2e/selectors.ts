export const sel = {
  trackList: "#track-list",
  trackRow: ".track-row",
  trackRowById: (id: number) => `[data-track-id="${id}"]`,
  trackRowCue: ".btn-cue",
  trackRowAdd: ".btn-add",
  searchInput: "#search-input",
  libraryTab: (label: string) => `[role="tab"][aria-selected]*=${label}`,
  sortHeader: (label: string) => `button[role="columnheader"]*=${label}`,
  settingsButton: "#btn-settings",
  scanButton: "#btn-scan-now",
  scanStatusBar: "#scan-status-bar",
  closeSettings: "#btn-close-settings",
  playlist: "#playlist",
  playlistRow: ".playlist-row",
  playlistRowDuration: ".pl-duration",
  /** Marks a queued item airing under cue points of its own. */
  playlistRowOverride: ".pl-override",
  clearPlaylist: "#btn-clear-playlist",
  autoPlaylistToggle: "#btn-generate",
  btnPlay: "#btn-play",
  btnStop: "#btn-stop",
  btnNext: "#btn-next",
  btnPrev: "#btn-prev",

  /**
   * Scoped to the main deck: `.time-pill` alone also matches the cue deck's,
   * and only resolved to this one because `#now-playing` comes first.
   */
  timeDisplay: "#progress-bar .time-pill",
  npTitle: "#np-title",
  editButton: ".btn-edit",
  contextMenu: "#context-menu",

  /**
   * Scope this from the menu element: WebdriverIO's `*=` text selectors
   * accept only a tag with one class/id/attribute, never a descendant
   * combinator.
   */
  contextMenuItem: (label: string) => `button[role="menuitem"]*=${label}`,
  /** Metadata only — playback markers have their own dialog. */
  metadataDialog: "#metadata-dialog",
  metadataTitle: "#metadata-title",
  metadataArtist: "#metadata-artist",
  metadataAlbum: "#metadata-album",
  metadataGenre: "#metadata-genre",
  metadataYear: "#metadata-year",
  metadataError: "#metadata-error",
  metadataSave: "#btn-metadata-save",
  metadataCancel: "#btn-metadata-cancel",
  cuePointDialog: "#cue-point-dialog",
  /** A marker's time field: `m:ss.mmm`, committed on Enter or blur. */
  cuePointField: (marker: string) => `#cue-field-${marker}`,
  /** Marker lines in the zoomed strip — one per set marker in view. */
  cuePointMarker: "#cue-point-dialog .cue-detail-line",
  /** Plays the raw file, markers ignored. */
  cuePointPlay: "#btn-cue-points-play",
  cuePointClock: "#cue-points-clock",
  cuePointSave: "#btn-cue-points-save",
  cuePointClose: "#btn-cue-points-close",
  cuePointCancel: "#btn-cue-points-cancel",
  cuePointDiscard: "#btn-cue-points-discard",
  /** Plays the draft on the cue deck without leaving the dialog. */
  cuePointAudition: "#btn-cue-points-audition",
  cuePointStop: "#btn-cue-points-stop",
  /** Queues the draft as a single airing, leaving the track untouched. */
  cuePointUseOnce: "#btn-cue-points-use-once",
  cuePointClearAll: "#btn-cue-points-clear",

  // Cue deck. Hidden entirely until an output device is picked in Settings, so
  // a spec that touches any of these has to enable it first.
  cueDeck: "#cue-deck",
  cueTimeDisplay: "#cue-deck .time-pill",
  cuePlay: "#cue-deck .btn-cue-play",
  cueStop: "#cue-deck .btn-cue-stop",
  cueEditPoints: "#cue-deck .btn-cue-points",
  cuePromote: "#cue-deck .btn-cue-promote",
  /** Scoped: the main deck has its own `.segmented` group (Auto/Manual). */
  cueModeActive: '#cue-deck .segmented button[aria-pressed="true"]',

  /**
   * Settings tabs carry no ids; WebdriverIO's `*=` accepts a tag plus one
   * class.
   */
  settingsTab: (label: string) => `button.settings-tab*=${label}`,
  cueDeviceSelect: "#cue-device",
  themeRow: (id: string) => `#appearance-theme-${id}`,
  reloadThemes: "#appearance-reload",
} as const;
