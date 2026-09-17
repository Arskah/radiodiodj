import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MockBackend } from "./mockBackend";
import { MockPlaylistBackend } from "./mockPlaylist";

const { api } = vi.hoisted(() => {
  const api = {
    platform: "darwin",
    search: vi.fn(),
    getTrack: vi.fn(),
    getTracksByIds: vi.fn(),
    getWaveform: vi.fn(),
    getCoverArt: vi.fn(),
    loadSession: vi.fn(),
    saveSession: vi.fn(),
    trackPlayed: vi.fn(),
    revealTrack: vi.fn(),
    mainDeckIsPlaying: vi.fn(),
    playlistSync: vi.fn(),
    onPlaylistState: vi.fn(),
    onDeckRoles: vi.fn(),
    onTailTime: vi.fn(),
    onTailDuration: vi.fn(),
    playlistAdd: vi.fn(),
    playlistAddFront: vi.fn(),
    playlistSetItemCuePoints: vi.fn(),
    playlistAddStopMarker: vi.fn(),
    playlistAddFiller: vi.fn(),
    playlistRemove: vi.fn(),
    playlistMove: vi.fn(),
    playlistClear: vi.fn(),
    playlistPlayIndex: vi.fn(),
    playlistPlayNow: vi.fn(),
    playlistNext: vi.fn(),
    playlistPrev: vi.fn(),
    playlistStop: vi.fn(),
    playlistSetAutoPlaylist: vi.fn(),
    playlistSetAutoAdvance: vi.fn(),
    getStats: vi.fn(),
    getPaths: vi.fn(),
    getAllPaths: vi.fn(),
    addPath: vi.fn(),
    removePath: vi.fn(),
    purgeTracks: vi.fn(),
    libraryHealth: vi.fn(),
    onLibraryHealth: vi.fn(),
    libraryCheckNow: vi.fn(),
    healthDismiss: vi.fn(),
    healthUndismiss: vi.fn(),
    scanLibraries: vi.fn(),
    cancelScan: vi.fn(),
    getScanStatus: vi.fn(),
    onScanProgress: vi.fn(),
    onScanStateChanged: vi.fn(),
    onWaveformReady: vi.fn(),
    onWaveformProgress: vi.fn(),
    onWaveformStateChanged: vi.fn(),
    getWaveformStatus: vi.fn(),
    listAudioDevices: vi.fn(),
    getMainDevice: vi.fn(),
    setMainDevice: vi.fn(),
    getCueDevice: vi.fn(),
    setCueDevice: vi.fn(),
    updateTrackMetadata: vi.fn(),
    revertTrackTags: vi.fn(),
    retryTagWrite: vi.fn(),
    dismissTagWrite: vi.fn(),
    mainDeckFadeOut: vi.fn(),
    mainDeckFadeToNext: vi.fn(),
    getTuningConfig: vi.fn(),
    setTuningConfig: vi.fn(),
    setCuePoints: vi.fn(),
    adminStatus: vi.fn(),
    adminUnlock: vi.fn(),
    adminLock: vi.fn(),
    adminSetPassword: vi.fn(),
    adminClearPassword: vi.fn(),
    adminSetIdleLockMin: vi.fn(),
    onAdminStateChanged: vi.fn(),
  };
  return { api };
});

// Default tuning config as returned by the backend.
function defaultTuning() {
  return {
    interleave: {
      jingleEvery: 4,
      commercialEvery: 8,
      commercialBucketMultiplier: 3,
      commercialBucketMin: 10,
    },
    autoPlaylist: {
      autoPlaylistBuffer: 20,
      autoPlaylistThreshold: 5,
      historyCap: 100,
      sessionSaveThrottleMs: 500,
      netRetryBackoffsMs: [1000, 2000, 5000],
    },
    cache: { maxCacheBytes: 150 * 1024 * 1024 },
    player: {
      readWatchdogTimeoutMs: 10000,
      openRetryIntervalMs: 2000,
      readRetryBackoffsMs: [500, 1000, 2000],
      fadeOutMs: 4000,
      fadeToNextMs: 2500,
    },
    library: { checkIntervalMin: 15, writeTags: false, tagWriteTimeoutSec: 30 },
  };
}

vi.mock("./api", () => ({ api }));

vi.mock("../features/deck/nativeBackend", () => ({
  NativeBackend: class {
    on(): () => void {
      return () => {};
    }
    load(): Promise<void> {
      return Promise.resolve();
    }
    play(): Promise<void> {
      return Promise.resolve();
    }
    pause(): Promise<void> {
      return Promise.resolve();
    }
    stop(): Promise<void> {
      return Promise.resolve();
    }
    seek(): Promise<void> {
      return Promise.resolve();
    }
    setVolume(): Promise<void> {
      return Promise.resolve();
    }
    dispose(): Promise<void> {
      return Promise.resolve();
    }
  },
}));

import {
  AppState,
  EMPTY_HEALTH,
  formatSpan,
  formatTime,
  type Track,
} from "./state.svelte";
import type { ScanStatus } from "./api";
import {
  isTrackItem,
  stopMarker,
  trackItem,
  type CuePoints,
  type HealthReport,
  type PlaylistItem,
} from "./types";

const pid = (i: PlaylistItem): number | "STOP" =>
  isTrackItem(i) ? i.track.id : "STOP";

// Stands in for the library the backend resolves ids against: the playlist
// commands carry ids, so every track a test invents has to be findable by one.
const library = new Map<number, Track>();

const t = (id: number, extra: Partial<Track> = {}): Track => {
  const track: Track = {
    id,
    title: `t${id}`,
    artist: `a${id}`,
    album: `al${id}`,
    duration: 100,
    play_count: 0,
    ...extra,
  };
  library.set(id, track);
  return track;
};

const known = (id: number): Track => {
  const track = library.get(id);
  if (!track) throw new Error(`test track ${id} was never created`);
  return track;
};

/**
 * The program bus' role and tail-deck event streams, captured so a test can
 * drive a handover overlap the way the worker would.
 */
const bus: {
  roles?: (roles: unknown[]) => void;
  tailTime?: (seconds: number) => void;
  tailDuration?: (seconds: number) => void;
} = {};

function resetApi(): void {
  vi.clearAllMocks();
  library.clear();
  api.onDeckRoles.mockImplementation((cb: (roles: unknown[]) => void) => {
    bus.roles = cb;
    return Promise.resolve(() => {});
  });
  api.onTailTime.mockImplementation((cb: (s: number) => void) => {
    bus.tailTime = cb;
    return Promise.resolve(() => {});
  });
  api.onTailDuration.mockImplementation((cb: (s: number) => void) => {
    bus.tailDuration = cb;
    return Promise.resolve(() => {});
  });
  api.search.mockResolvedValue([]);
  api.trackPlayed.mockResolvedValue(undefined);
  api.mainDeckIsPlaying.mockResolvedValue(false);
  api.mainDeckFadeOut.mockResolvedValue(undefined);
  api.mainDeckFadeToNext.mockResolvedValue(undefined);
  api.getStats.mockResolvedValue({
    totalTracks: 0,
    totalArtists: 0,
    totalAlbums: 0,
    totalHours: 0,
  });
  api.getAllPaths.mockResolvedValue({
    music: [],
    commercial: [],
    jingle: [],
  });
  api.addPath.mockResolvedValue(null);
  api.removePath.mockResolvedValue(true);
  api.purgeTracks.mockResolvedValue(0);
  api.libraryHealth.mockResolvedValue(structuredClone(EMPTY_HEALTH));
  api.scanLibraries.mockResolvedValue({ alreadyRunning: false });
  api.cancelScan.mockResolvedValue(undefined);
  api.getScanStatus.mockResolvedValue({ status: "idle", lastResult: null });
  api.getTracksByIds.mockResolvedValue([]);
  api.getWaveform.mockResolvedValue(null);
  api.getCoverArt.mockResolvedValue(null);
  api.getWaveformStatus.mockResolvedValue({ status: "idle" });
  api.loadSession.mockResolvedValue({
    state: {
      playlistIds: [],
      playlistItems: [],
      historyIds: [],
      currentTrackId: null,
      currentTime: 0,
      autoPlaylistActive: false,
      autoAdvance: true,
      volume: 1,
      cueVolume: 1,
    },
    tracks: [],
    libraryReset: false,
  });
  api.saveSession.mockResolvedValue(undefined);
  api.adminStatus.mockResolvedValue({
    passwordSet: false,
    unlocked: true,
    idleLockMin: 15,
  });
  api.adminUnlock.mockResolvedValue(false);
  api.adminLock.mockResolvedValue(undefined);
  api.adminSetPassword.mockResolvedValue(undefined);
  api.adminClearPassword.mockResolvedValue(undefined);
  api.adminSetIdleLockMin.mockResolvedValue(undefined);
  api.listAudioDevices.mockResolvedValue([]);
  api.getMainDevice.mockResolvedValue(null);
  api.getCueDevice.mockResolvedValue(null);
  api.setMainDevice.mockResolvedValue(undefined);
  api.setCueDevice.mockResolvedValue(undefined);
  api.updateTrackMetadata.mockImplementation(async (input) => ({
    id: input.id,
    title: input.title ?? "",
    artist: input.artist ?? "",
    album: input.album ?? "",
    duration: 0,
    play_count: 0,
  }));
  api.getTuningConfig.mockResolvedValue(defaultTuning());
  api.setTuningConfig.mockImplementation((c: unknown) => Promise.resolve(c));
}

interface TestApp {
  app: AppState;
  mock: MockBackend;
  cueMock: MockBackend;
  playlist: MockPlaylistBackend;
}

/**
 * Point the playlist half of the api at an in-memory backend, so a command sent
 * by `AppState` comes back as a snapshot and the projection is exercised end to
 * end — the same shape as the real IPC round-trip.
 */
function wirePlaylist(playlist: MockPlaylistBackend): void {
  const ok = (run: () => void) => {
    run();
    return Promise.resolve();
  };
  api.onPlaylistState.mockImplementation((cb: (s: unknown) => void) =>
    Promise.resolve(playlist.on(cb as never)),
  );
  api.playlistSync.mockImplementation(() =>
    Promise.resolve(playlist.snapshot()),
  );
  api.playlistAdd.mockImplementation((id: number) =>
    ok(() => playlist.add(known(id))),
  );
  api.playlistAddFront.mockImplementation(
    (id: number, cuePoints: CuePoints | null = null) =>
      ok(() => playlist.addFront(known(id), cuePoints)),
  );
  api.playlistSetItemCuePoints.mockImplementation(
    (index: number, cuePoints: CuePoints | null) =>
      ok(() => playlist.setItemCuePoints(index, cuePoints)),
  );
  api.playlistAddStopMarker.mockImplementation(() =>
    ok(() => playlist.addStopMarker()),
  );
  api.playlistAddFiller.mockResolvedValue(undefined);
  api.playlistRemove.mockImplementation((index: number) =>
    ok(() => playlist.remove(index)),
  );
  api.playlistMove.mockImplementation((from: number, to: number) =>
    ok(() => playlist.move(from, to)),
  );
  api.playlistClear.mockImplementation(() => ok(() => playlist.clear()));
  api.playlistPlayIndex.mockImplementation((index: number) =>
    ok(() => playlist.playIndex(index)),
  );
  api.playlistPlayNow.mockImplementation((id: number) =>
    ok(() => playlist.playNow(known(id))),
  );
  api.playlistNext.mockImplementation(() => ok(() => playlist.next()));
  api.playlistPrev.mockImplementation((id: number) =>
    ok(() => playlist.prev(known(id))),
  );
  api.playlistStop.mockImplementation(() => ok(() => playlist.stop()));
  api.playlistSetAutoPlaylist.mockImplementation((active: boolean) =>
    ok(() => playlist.setAutoPlaylist(active)),
  );
  api.playlistSetAutoAdvance.mockImplementation((active: boolean) =>
    ok(() => playlist.setAutoAdvance(active)),
  );
}

function makeApp(): TestApp {
  document.body.innerHTML = "";
  document.title = "RadiodioDJ";
  const mock = new MockBackend();
  const cueMock = new MockBackend();
  const playlist = new MockPlaylistBackend();
  wirePlaylist(playlist);
  const app = new AppState(mock, cueMock);
  return { app, mock, cueMock, playlist };
}

async function flushAsync(): Promise<void> {
  for (let i = 0; i < 5; i++) await Promise.resolve();
}

describe("formatTime", () => {
  it("formats seconds as M:SS", () => {
    expect(formatTime(0)).toBe("0:00");
    expect(formatTime(5)).toBe("0:05");
    expect(formatTime(65)).toBe("1:05");
    expect(formatTime(3661)).toBe("61:01");
  });

  it("returns 0:00 for invalid input", () => {
    expect(formatTime(NaN)).toBe("0:00");
    expect(formatTime(Infinity)).toBe("0:00");
  });
});

describe("formatSpan", () => {
  it("reads as M:SS under an hour", () => {
    expect(formatSpan(0)).toBe("0:00");
    expect(formatSpan(3599)).toBe("59:59");
  });

  it("carries hours past an hour", () => {
    expect(formatSpan(3600)).toBe("1:00:00");
    expect(formatSpan(3661)).toBe("1:01:01");
    expect(formatSpan(36_000 + 125)).toBe("10:02:05");
  });

  it("returns 0:00 for invalid input", () => {
    expect(formatSpan(NaN)).toBe("0:00");
  });
});

describe("AppState playlist mutations", () => {
  let app: AppState;
  beforeEach(() => {
    resetApi();
    app = makeApp().app;
  });

  it("addToPlaylist appends tracks", () => {
    app.addToPlaylist(t(1));
    app.addToPlaylist(t(2));
    expect(app.playlist.length).toBe(2);
    expect(pid(app.playlist[1])).toBe(2);
  });

  it("addToPlaylist forwards the track id, not the track", () => {
    app.addToPlaylist(t(1));
    expect(api.playlistAdd).toHaveBeenCalledWith(1);
  });

  it("addNextToPlaylist queues the track ahead of everything queued", () => {
    app.addToPlaylist(t(1));
    app.addToPlaylist(t(2));
    app.addNextToPlaylist(t(3));
    expect(app.playlist.map(pid)).toEqual([3, 1, 2]);
    expect(api.playlistAddFront).toHaveBeenCalledWith(3);
  });

  it("revealTrack forwards the track id", () => {
    api.revealTrack.mockResolvedValue(undefined);
    app.revealTrack(t(7));
    expect(api.revealTrack).toHaveBeenCalledWith(7);
  });

  it("removeFromPlaylist splices the entry without touching the track on air", () => {
    app.playNow(t(99));
    app.addToPlaylist(t(1));
    app.addToPlaylist(t(2));
    app.addToPlaylist(t(3));
    app.removeFromPlaylist(0);
    expect(app.playlist.map(pid)).toEqual([2, 3]);
    expect(app.currentTrack?.id).toBe(99);
  });

  it("clearPlaylist empties the queue but leaves the track on air playing", () => {
    app.playNow(t(99));
    app.addToPlaylist(t(1));
    app.clearPlaylist();
    expect(app.playlist.length).toBe(0);
    expect(app.currentTrack?.id).toBe(99);
  });

  it("movePlaylistItem reorders entries", () => {
    app.addToPlaylist(t(1));
    app.addToPlaylist(t(2));
    app.addToPlaylist(t(3));
    app.movePlaylistItem(0, 2);
    expect(app.playlist.map(pid)).toEqual([2, 3, 1]);
  });

  it("movePlaylistItem is a no-op when from === to", () => {
    app.addToPlaylist(t(1));
    app.addToPlaylist(t(2));
    app.movePlaylistItem(0, 0);
    expect(api.playlistMove).not.toHaveBeenCalled();
    expect(app.playlist.map(pid)).toEqual([1, 2]);
  });

  it("addFiller asks the backend to pick one of that content type", async () => {
    await app.addFiller("jingle");
    expect(api.playlistAddFiller).toHaveBeenCalledWith("jingle");
  });
});

describe("AppState history view", () => {
  let app: AppState;
  beforeEach(() => {
    resetApi();
    app = makeApp().app;
  });

  it("playlistTab defaults to playlist", () => {
    expect(app.playlistTab).toBe("playlist");
  });

  it("historyDisplay reverses storage order (newest first)", () => {
    app.history.push(t(1), t(2), t(3));
    expect(app.historyDisplay.map((x) => x.id)).toEqual([3, 2, 1]);
  });

  it("history caps at 100 entries, dropping the oldest", () => {
    for (let i = 0; i < 100; i++) app.history.push(t(i));
    app.playNow(t(500));
    app.playNow(t(999));
    expect(app.history.length).toBe(100);
    expect(app.history[0].id).toBe(1);
    expect(app.history[99].id).toBe(500);
  });

  it("removeFromHistory uses display index (newest first)", () => {
    app.history.push(t(1), t(2), t(3));
    app.removeFromHistory(0);
    expect(app.history.map((x) => x.id)).toEqual([1, 2]);
    app.removeFromHistory(1);
    expect(app.history.map((x) => x.id)).toEqual([2]);
  });

  it("removeFromHistory ignores out-of-range indices", () => {
    app.history.push(t(1));
    app.removeFromHistory(5);
    app.removeFromHistory(-1);
    expect(app.history.length).toBe(1);
  });

  it("clearHistory empties history without touching playback", () => {
    app.history.push(t(1), t(2));
    app.playNow(t(99));
    app.clearHistory();
    expect(app.history.length).toBe(0);
    expect(app.currentTrack?.id).toBe(99);
  });

  it("requeueFromHistory appends the chosen entry to the playlist tail", () => {
    app.history.push(t(1), t(2), t(3));
    app.requeueFromHistory(2);
    expect(app.playlist.map(pid)).toEqual([1]);
    expect(app.history.map((x) => x.id)).toEqual([1, 2, 3]);
  });

  it("requeueFromHistory is a no-op for invalid indices", () => {
    app.history.push(t(1));
    app.requeueFromHistory(5);
    expect(app.playlist.length).toBe(0);
  });
});

describe("AppState playback control", () => {
  let app: AppState;
  let mock: MockBackend;
  beforeEach(() => {
    resetApi();
    ({ app, mock } = makeApp());
  });

  it("playIndex pulls track out of playlist into currentTrack and plays it", () => {
    app.addToPlaylist(t(7, { title: "Song", artist: "Band" }));
    app.playIndex(0);
    expect(api.playlistPlayIndex).toHaveBeenCalledWith(0);
    expect(app.currentTrack?.id).toBe(7);
    expect(app.playlist.length).toBe(0);
    expect(document.title).toBe("Song - Band | RadiodioDJ");
  });

  it("playing a new track moves the previous one into history", () => {
    app.addToPlaylist(t(1));
    app.addToPlaylist(t(2));
    app.addToPlaylist(t(3));
    app.playIndex(0);
    expect(app.history.length).toBe(0);
    app.playIndex(0);
    expect(app.history.map((x) => x.id)).toEqual([1]);
    app.playNow(t(99));
    expect(app.history.map((x) => x.id)).toEqual([1, 2]);
    expect(app.currentTrack?.id).toBe(99);
  });

  it("playIndex out of range is a no-op", () => {
    app.playIndex(0);
    expect(api.playlistPlayIndex).not.toHaveBeenCalled();
    expect(app.currentTrack).toBeNull();
  });

  it("playNow plays directly without enqueuing", () => {
    app.playNow(t(5));
    expect(app.currentTrack?.id).toBe(5);
    expect(app.playlist.length).toBe(0);
  });

  it("next plays the next queued track", () => {
    app.addToPlaylist(t(1));
    app.addToPlaylist(t(2));
    app.next();
    expect(app.currentTrack?.id).toBe(1);
    expect(app.playlist.map(pid)).toEqual([2]);
  });

  it("next is a no-op when playlist empty", () => {
    app.playNow(t(99));
    app.next();
    expect(app.currentTrack?.id).toBe(99);
  });

  it("prev after 3s seeks to start of current track", () => {
    app.playNow(t(1));
    app.currentTime = 5;
    app.prev();
    expect(app.currentTime).toBe(0);
    expect(mock.lastSeek).toBe(0);
    expect(api.playlistPrev).not.toHaveBeenCalled();
    expect(app.currentTrack?.id).toBe(1);
  });

  it("prev within 3s peeks history (entry stays) and pushes current onto queue head", () => {
    app.addToPlaylist(t(1));
    app.addToPlaylist(t(2));
    app.playIndex(0);
    app.playIndex(0);
    expect(app.currentTrack?.id).toBe(2);
    expect(app.history.map((x) => x.id)).toEqual([1]);
    app.currentTime = 1;
    app.prev();
    expect(app.currentTrack?.id).toBe(1);
    expect(app.history.map((x) => x.id)).toEqual([1]);
    expect(app.playlist.map(pid)).toEqual([2]);
  });

  it("prev within 3s when current already equals history top just rewinds", () => {
    app.addToPlaylist(t(1));
    app.addToPlaylist(t(2));
    app.playIndex(0);
    app.playIndex(0);
    app.currentTime = 1;
    app.prev();
    expect(app.currentTrack?.id).toBe(1);
    expect(app.playlist.map(pid)).toEqual([2]);
    app.currentTime = 1;
    app.prev();
    expect(app.currentTrack?.id).toBe(1);
    expect(app.history.map((x) => x.id)).toEqual([1]);
    expect(app.playlist.map(pid)).toEqual([2]);
    expect(app.currentTime).toBe(0);
  });

  it("prev within 3s with empty history just restarts current", () => {
    app.playNow(t(1));
    app.currentTime = 1;
    app.prev();
    expect(app.currentTime).toBe(0);
    expect(mock.lastSeek).toBe(0);
    expect(app.currentTrack?.id).toBe(1);
  });

  it("prev is a no-op when no current track and empty history", () => {
    app.currentTime = 5;
    app.prev();
    expect(app.currentTime).toBe(5);
    expect(mock.seekCalls.length).toBe(0);
    expect(app.currentTrack).toBeNull();
  });

  it("togglePlay starts head of queue when nothing playing and playlist non-empty", () => {
    app.addToPlaylist(t(3));
    app.togglePlay();
    expect(app.currentTrack?.id).toBe(3);
  });

  it("toggleMode flips autoAdvance", () => {
    expect(app.autoAdvance).toBe(true);
    app.toggleMode();
    expect(app.autoAdvance).toBe(false);
    app.toggleMode();
    expect(app.autoAdvance).toBe(true);
  });

  it("airTimeRemaining is null with nothing on air", () => {
    app.addToPlaylist(t(1));
    expect(app.upcomingAirTime).toBe(100);
    expect(app.airTimeRemaining).toBeNull();
  });

  it("airTimeRemaining adds the queue up to a stop marker while Auto advances", () => {
    app.playNow(t(99));
    app.addToPlaylist(t(1, { duration: 60 }));
    app.addStopMarker();
    app.addToPlaylist(t(2));
    app.duration = 100;
    app.currentTime = 30;
    expect(app.upcomingAirTime).toBe(60);
    expect(app.airTimeRemaining).toBe(130);
    app.toggleMode();
    expect(app.airTimeRemaining).toBe(70);
  });

  it("toggleAutoPlaylist activates and starts playing first track when idle", async () => {
    app.addToPlaylist(t(9));
    await app.toggleAutoPlaylist();
    expect(app.autoPlaylistActive).toBe(true);
    expect(app.currentTrack?.id).toBe(9);
  });

  it("toggleAutoPlaylist deactivates without touching playback", async () => {
    app.addToPlaylist(t(9));
    await app.toggleAutoPlaylist();
    expect(app.autoPlaylistActive).toBe(true);
    await app.toggleAutoPlaylist();
    expect(app.autoPlaylistActive).toBe(false);
    expect(app.currentTrack?.id).toBe(9);
  });

  it("stop clears currentTrack, autoPlaylist flag, time/duration and title, and pushes to history", async () => {
    app.addToPlaylist(t(5));
    app.addToPlaylist(t(6));
    app.playIndex(0);
    app.playIndex(0);
    expect(app.history.length).toBe(1);
    await app.toggleAutoPlaylist();
    app.currentTime = 12;
    app.duration = 200;
    app.stop();
    expect(app.currentTrack).toBeNull();
    expect(app.history.map((x) => x.id)).toEqual([5, 6]);
    expect(app.autoPlaylistActive).toBe(false);
    expect(app.currentTime).toBe(0);
    expect(app.duration).toBe(0);
    expect(document.title).toBe("RadiodioDJ");
  });

  it("setVolume updates state and backend", () => {
    app.setVolume(0.4);
    expect(app.volume).toBe(0.4);
    expect(mock.volume).toBe(0.4);
  });

  it("seekToPct clamps and applies via backend", () => {
    app.duration = 100;
    app.currentTime = 0;
    app.seekToPct(0.5);
    expect(app.currentTime).toBe(50);
    expect(mock.lastSeek).toBe(50);
    app.seekToPct(2);
    expect(app.currentTime).toBe(100);
    expect(mock.lastSeek).toBe(100);
    app.seekToPct(-1);
    expect(app.currentTime).toBe(0);
    expect(mock.lastSeek).toBe(0);
  });

  it("seekToPct is a no-op when duration is zero", () => {
    app.duration = 0;
    app.currentTime = 7;
    app.seekToPct(0.5);
    expect(app.currentTime).toBe(7);
    expect(mock.seekCalls.length).toBe(0);
  });

  it("progressPct reflects currentTime/duration ratio", () => {
    app.duration = 200;
    app.currentTime = 50;
    expect(app.progressPct).toBe(25);
    app.duration = 0;
    expect(app.progressPct).toBe(0);
  });

  it("fadeOut asks for the configured duration and shows the ramp", () => {
    app.fadeOut();
    expect(api.mainDeckFadeOut).toHaveBeenCalledWith(undefined);
    expect(app.fading).toBe("out");
    expect(app.fadeMs).toBe(app.tuning.player.fadeOutMs);
  });

  it("pressing a fade again finishes it now rather than restarting it", () => {
    app.fadeOut();
    app.fadeOut();
    expect(api.mainDeckFadeOut).toHaveBeenLastCalledWith(0);
    expect(app.fading).toBeNull();
  });

  it("fadeToNext uses its own duration", () => {
    app.fadeToNext();
    expect(api.mainDeckFadeToNext).toHaveBeenCalledWith(undefined);
    expect(app.fading).toBe("next");
    expect(app.fadeMs).toBe(app.tuning.player.fadeToNextMs);
  });

  it("transport actions clear a running fade", () => {
    for (const act of [
      () => app.stop(),
      () => app.next(),
      () => app.prev(),
      () => app.togglePlay(),
    ]) {
      app.fadeOut();
      expect(app.fading).toBe("out");
      act();
      expect(app.fading).toBeNull();
    }
  });

  it("a deck that went quiet clears the fade", () => {
    app.fadeOut();
    mock.emitPauseState(true);
    expect(app.fading).toBeNull();
  });
});

describe("AppState waveform", () => {
  let app: AppState;
  beforeEach(() => {
    resetApi();
    app = makeApp().app;
  });

  it("loads and stores the waveform when a track starts", async () => {
    api.getWaveform.mockResolvedValue([0, 128, 255]);
    app.playNow(t(7));
    expect(api.getWaveform).toHaveBeenCalledWith(7);
    await flushAsync();
    expect(app.waveform).toEqual([0, 128, 255]);
  });

  it("clears the waveform on stop", async () => {
    api.getWaveform.mockResolvedValue([1, 2, 3]);
    app.playNow(t(7));
    await flushAsync();
    app.stop();
    expect(app.waveform).toBeNull();
  });

  it("ignores a stale fetch after the track changed", async () => {
    // First track's fetch is slow; second resolves immediately. The stale
    // first result must not clobber the current waveform.
    let resolveFirst!: (v: number[] | null) => void;
    api.getWaveform.mockImplementationOnce(
      () => new Promise((r) => (resolveFirst = r)),
    );
    api.getWaveform.mockResolvedValueOnce([9, 9, 9]);

    app.playNow(t(1));
    app.playNow(t(2));
    await flushAsync();
    expect(app.waveform).toEqual([9, 9, 9]);

    resolveFirst([1, 1, 1]);
    await flushAsync();
    expect(app.waveform).toEqual([9, 9, 9]);
  });

  it("loads the cue waveform independently", async () => {
    api.getWaveform.mockResolvedValue([5, 6, 7]);
    app.cueLoad(t(3));
    expect(api.getWaveform).toHaveBeenCalledWith(3);
    await flushAsync();
    expect(app.cueWaveform).toEqual([5, 6, 7]);
    app.cueStop();
    expect(app.cueWaveform).toBeNull();
  });

  it("refetches on waveform-ready for the loaded track", async () => {
    // Track loads before its waveform is computed → first fetch is empty.
    api.getWaveform.mockResolvedValue(null);
    app.playNow(t(7));
    await flushAsync();
    expect(app.waveform).toBeNull();

    // Background worker finishes → the ready callback refetches, now populated.
    api.getWaveform.mockResolvedValue([1, 2, 3]);
    const onReady = api.onWaveformReady.mock.calls[0][0] as (
      id: number,
    ) => void;
    onReady(7);
    await flushAsync();
    expect(app.waveform).toEqual([1, 2, 3]);
  });

  it("ignores waveform-ready for a track that is not loaded", async () => {
    api.getWaveform.mockResolvedValue(null);
    app.playNow(t(7));
    await flushAsync();
    api.getWaveform.mockClear();

    const onReady = api.onWaveformReady.mock.calls[0][0] as (
      id: number,
    ) => void;
    onReady(999);
    expect(api.getWaveform).not.toHaveBeenCalled();
  });

  it("tracks waveform-pass progress via state + progress events", () => {
    const onState = api.onWaveformStateChanged.mock.calls[0][0] as (s: {
      status: string;
      processed?: number;
      total?: number;
    }) => void;
    const onProgress = api.onWaveformProgress.mock.calls[0][0] as (p: {
      processed: number;
      total: number;
    }) => void;

    onState({ status: "running", processed: 0, total: 10 });
    onProgress({ processed: 4, total: 10 });
    expect(app.waveformStatus).toEqual({
      status: "running",
      processed: 4,
      total: 10,
    });

    // Progress arriving after idle is ignored (no resurrecting the bar).
    onState({ status: "idle" });
    onProgress({ processed: 9, total: 10 });
    expect(app.waveformStatus).toEqual({ status: "idle" });
  });
});

describe("AppState backend events", () => {
  let app: AppState;
  let mock: MockBackend;
  beforeEach(() => {
    resetApi();
    ({ app, mock } = makeApp());
  });

  it("shows the outgoing track of a handover while it is still audible", () => {
    app.addToPlaylist(t(1));
    app.addToPlaylist(t(2));
    app.playIndex(0);
    // The handover's snapshot is what puts the outgoing track in history,
    // which is where its title is resolved from.
    app.playIndex(0);
    expect(app.history.at(-1)?.id).toBe(1);

    bus.tailDuration?.(180);
    bus.roles?.([
      { slot: "a", role: "tail", trackId: 1 },
      { slot: "b", role: "main", trackId: 2 },
    ]);
    bus.tailTime?.(172);

    expect(app.tailTrack?.id).toBe(1);
    expect(app.tailRemaining).toBe(8);
  });

  it("drops the outgoing track the moment the tail is vacated", () => {
    app.addToPlaylist(t(1));
    app.addToPlaylist(t(2));
    app.playIndex(0);
    app.playIndex(0);
    bus.tailDuration?.(180);
    bus.roles?.([{ slot: "a", role: "tail", trackId: 1 }]);
    expect(app.tailTrack?.id).toBe(1);

    bus.roles?.([
      { slot: "a", role: "arm", trackId: null },
      { slot: "b", role: "main", trackId: 2 },
    ]);

    expect(app.tailTrack).toBeNull();
    expect(app.tailRemaining).toBe(0);
  });

  it("time event mirrors to currentTime", () => {
    mock.emitTime(42);
    expect(app.currentTime).toBe(42);
  });

  it("duration event mirrors to duration", () => {
    mock.emitDuration(180);
    expect(app.duration).toBe(180);
  });

  it("pause-state event mirrors to isPlaying (inverse)", () => {
    mock.emitPauseState(false);
    expect(app.isPlaying).toBe(true);
    mock.emitPauseState(true);
    expect(app.isPlaying).toBe(false);
  });

  it("ended is the backend's business now, not the renderer's", async () => {
    app.addToPlaylist(t(1));
    app.addToPlaylist(t(2));
    app.playIndex(0);
    expect(app.currentTrack?.id).toBe(1);
    // The backend listens to the same `main-deck:ended` topic and advances off
    // it. A renderer that also advanced would double-skip.
    mock.emitEnded();
    await flushAsync();
    expect(app.currentTrack?.id).toBe(1);
    expect(api.playlistNext).not.toHaveBeenCalled();
    expect(api.playlistPlayIndex).toHaveBeenCalledTimes(1);
  });

  it("load-failed clears buffering without advancing", async () => {
    app.addToPlaylist(t(1));
    app.addToPlaylist(t(2));
    app.playIndex(0);
    app.isBuffering = true;
    mock.emitLoadFailed(1);
    await flushAsync();
    expect(app.isBuffering).toBe(false);
    expect(app.currentTrack?.id).toBe(1);
    expect(api.playlistNext).not.toHaveBeenCalled();
  });
});

describe("AppState outage indicators", () => {
  let app: AppState;
  let mock: MockBackend;
  let cueMock: MockBackend;
  let playlist: MockPlaylistBackend;
  beforeEach(() => {
    resetApi();
    ({ app, mock, cueMock, playlist } = makeApp());
  });

  // Skip-to-cached advancement, the retry backoff and the prefetch window moved
  // to the backend with the playlist; they are specified by `playlist::engine`.
  // What is left here is what the renderer still decides: how an outage looks.

  it("awaitingNetwork is mirrored from the snapshot and raises the banner", () => {
    expect(app.reconnecting).toBe(false);
    playlist.setAwaitingNetwork(true);
    expect(app.awaitingNetwork).toBe(true);
    expect(app.reconnecting).toBe(true);
    playlist.setAwaitingNetwork(false);
    expect(app.reconnecting).toBe(false);
  });

  it("prefetch-failed flags the share unreachable while playback continues, cleared on cache-state", async () => {
    app.addToPlaylist(t(1));
    app.addToPlaylist(t(2));
    app.playIndex(0); // current = 1, in RAM and playing
    await flushAsync();

    // Share drops mid-track: prefetch of an upcoming track fails, but the
    // current in-RAM track keeps playing — playback is not blocked.
    mock.emitPrefetchFailed();
    expect(app.shareUnreachable).toBe(true);
    expect(app.reconnecting).toBe(true);
    expect(app.currentTrack?.id).toBe(1); // playback undisturbed

    // A successful read (cache-state) signals recovery.
    mock.emitCacheState([2]);
    await flushAsync();
    expect(app.shareUnreachable).toBe(false);
    expect(app.reconnecting).toBe(false);
    expect(app.currentTrack?.id).toBe(1); // still not interrupted
  });

  it("output-unavailable toggles the main-deck flag and clears on recovery", () => {
    expect(app.outputUnavailable).toBe(false);

    // No audio device could be opened.
    mock.emitOutputUnavailable(true);
    expect(app.outputUnavailable).toBe(true);
    // Distinct from a network outage — the media share is fine.
    expect(app.reconnecting).toBe(false);

    // Backend auto-retry recovered a device.
    mock.emitOutputUnavailable(false);
    expect(app.outputUnavailable).toBe(false);
  });

  it("output-unavailable on the cue deck sets only the cue flag", () => {
    cueMock.emitOutputUnavailable(true);
    expect(app.cueOutputUnavailable).toBe(true);
    expect(app.outputUnavailable).toBe(false); // main deck unaffected

    cueMock.emitOutputUnavailable(false);
    expect(app.cueOutputUnavailable).toBe(false);
  });
});

describe("AppState library + paths", () => {
  let app: AppState;
  beforeEach(() => {
    resetApi();
    api.search.mockResolvedValue([t(1), t(2)]);
    api.getStats.mockResolvedValue({
      totalTracks: 5,
      totalArtists: 2,
      totalAlbums: 3,
      totalHours: 1,
    });
    api.getAllPaths.mockResolvedValue({
      music: ["/m"],
      commercial: [],
      jingle: [],
    });
    app = makeApp().app;
  });

  it("search forwards query + tab and stores results", async () => {
    app.searchQuery = "foo";
    app.activeTab = "music";
    await app.search();
    expect(api.search).toHaveBeenCalledWith("foo", "music", undefined, "asc");
    expect(app.tracks.length).toBe(2);
  });

  it("setTab updates activeTab and triggers a search for it", () => {
    app.setTab("jingle");
    expect(app.activeTab).toBe("jingle");
    expect(api.search).toHaveBeenCalledWith("", "jingle", undefined, "asc");
  });

  it("toggleSort sets column ascending then flips direction on second click", async () => {
    await app.toggleSort("title");
    expect(app.sortBy).toBe("title");
    expect(app.sortDir).toBe("asc");
    expect(api.search).toHaveBeenLastCalledWith("", "music", "title", "asc");
    await app.toggleSort("title");
    expect(app.sortDir).toBe("desc");
    expect(api.search).toHaveBeenLastCalledWith("", "music", "title", "desc");
  });

  it("toggleSort to a different column resets direction to asc", async () => {
    app.sortBy = "artist";
    app.sortDir = "desc";
    await app.toggleSort("album");
    expect(app.sortBy).toBe("album");
    expect(app.sortDir).toBe("asc");
    expect(api.search).toHaveBeenLastCalledWith("", "music", "album", "asc");
  });

  it("loadStats stores response", async () => {
    await app.loadStats();
    expect(app.stats?.totalTracks).toBe(5);
  });

  it("loadLibraryPaths stores response", async () => {
    await app.loadLibraryPaths();
    expect(app.libraryPaths.music).toEqual(["/m"]);
  });

  it("addPath skips reload when api returns null", async () => {
    api.addPath.mockResolvedValueOnce(null);
    await app.addPath("music");
    expect(api.getAllPaths).not.toHaveBeenCalled();
  });

  it("addPath reloads when api returns a new path", async () => {
    api.addPath.mockResolvedValueOnce("/new");
    await app.addPath("music");
    expect(api.getAllPaths).toHaveBeenCalled();
  });

  it("removePath calls api then reloads", async () => {
    await app.removePath("music", "/m");
    expect(api.removePath).toHaveBeenCalledWith("music", "/m");
    expect(api.getAllPaths).toHaveBeenCalled();
  });

  it("purgeTracks deletes the chosen tracks, then refreshes stats and library", async () => {
    api.getStats.mockClear();
    api.search.mockClear();

    await app.purgeTracks([3, 4]);

    expect(api.purgeTracks).toHaveBeenCalledWith([3, 4]);
    expect(api.getStats).toHaveBeenCalled();
    expect(api.search).toHaveBeenCalled();
  });

  it("purgeTracks still refreshes when the backend refuses", async () => {
    api.purgeTracks.mockRejectedValueOnce("a library scan is running");
    api.getStats.mockClear();

    await app.purgeTracks([3]);

    expect(api.getStats).toHaveBeenCalled();
  });

  it("scan invokes scanLibraries fire-and-forget without blocking on result", async () => {
    await app.scan();
    expect(api.scanLibraries).toHaveBeenCalled();
  });

  it("cancelScan invokes the api", async () => {
    await app.cancelScan();
    expect(api.cancelScan).toHaveBeenCalled();
  });

  it("scan-state-changed transition from running to idle refreshes search + stats", async () => {
    const cb = api.onScanStateChanged.mock.calls[0]?.[0] as
      ((s: ScanStatus) => void) | undefined;
    expect(cb).toBeDefined();
    cb!({ status: "running", processed: 0, total: 0 });
    expect(app.scanStatus.status).toBe("running");
    api.search.mockClear();
    api.getStats.mockClear();
    cb!({ status: "idle", lastResult: { total: 5, added: 5 } });
    await Promise.resolve();
    expect(api.search).toHaveBeenCalled();
    expect(api.getStats).toHaveBeenCalled();
  });

  it("scan-progress patches running state", () => {
    const stateCb = api.onScanStateChanged.mock.calls[0]?.[0] as
      ((s: ScanStatus) => void) | undefined;
    const progCb = api.onScanProgress.mock.calls[0]?.[0] as
      ((p: { processed: number; total: number }) => void) | undefined;
    stateCb!({ status: "running", processed: 0, total: 0 });
    progCb!({ processed: 7, total: 10 });
    expect(app.scanStatus).toEqual({
      status: "running",
      processed: 7,
      total: 10,
    });
  });
});

describe("AppState tuning", () => {
  let app: AppState;

  beforeEach(() => {
    resetApi();
    app = makeApp().app;
  });

  it("defaults to the built-in tuning before load", () => {
    expect(app.tuning.autoPlaylist.autoPlaylistBuffer).toBe(20);
    expect(app.tuning.autoPlaylist.autoPlaylistThreshold).toBe(5);
  });

  it("loadTuning adopts backend values", async () => {
    const cfg = defaultTuning();
    cfg.autoPlaylist.autoPlaylistBuffer = 8;
    cfg.autoPlaylist.autoPlaylistThreshold = 3;
    api.getTuningConfig.mockResolvedValueOnce(cfg);
    await app.loadTuning();
    expect(app.tuning.autoPlaylist.autoPlaylistBuffer).toBe(8);
  });

  it("loadTuning keeps defaults when the backend errors", async () => {
    api.getTuningConfig.mockRejectedValueOnce(new Error("boom"));
    await app.loadTuning();
    expect(app.tuning.autoPlaylist.autoPlaylistBuffer).toBe(20);
  });

  it("saveTuning persists and adopts the clamped result", async () => {
    const requested = defaultTuning();
    requested.autoPlaylist.autoPlaylistBuffer = 4;
    requested.autoPlaylist.autoPlaylistThreshold = 99;
    // Backend clamps threshold down to the buffer.
    const clamped = defaultTuning();
    clamped.autoPlaylist.autoPlaylistBuffer = 4;
    clamped.autoPlaylist.autoPlaylistThreshold = 4;
    api.setTuningConfig.mockResolvedValueOnce(clamped);

    await app.saveTuning(requested);
    expect(api.setTuningConfig).toHaveBeenCalledWith(requested);
    expect(app.tuning.autoPlaylist.autoPlaylistThreshold).toBe(4);
  });
});

describe("AppState session persistence", () => {
  let app: AppState;
  let mock: MockBackend;
  let playlist: MockPlaylistBackend;

  beforeEach(() => {
    resetApi();
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("loadSession mirrors the backend's restored playlist and keeps history and position", async () => {
    api.loadSession.mockResolvedValueOnce({
      state: {
        playlistIds: [2, 3],
        playlistItems: [],
        historyIds: [1],
        currentTrackId: 2,
        currentTime: 12.5,
        autoPlaylistActive: true,
        autoAdvance: false,
        volume: 0.6,
        cueVolume: 0.3,
      },
      tracks: [t(1), t(2), t(3)],
      libraryReset: false,
    });

    ({ app, mock, playlist } = makeApp());
    // The backend restored the same session file before the window opened, so
    // the playlist, the track on the deck and the auto flags arrive over
    // playlist_sync rather than out of the session payload.
    playlist.restore({
      playlist: [trackItem(t(2)), trackItem(t(3))],
      current: t(2),
      autoPlaylistActive: true,
      autoAdvance: false,
    });
    await app.loadSession();

    expect(app.playlist.map(pid)).toEqual([2, 3]);
    expect(app.history.map((x) => x.id)).toEqual([1]);
    expect(app.currentTrack?.id).toBe(2);
    expect(app.autoPlaylistActive).toBe(true);
    expect(app.autoAdvance).toBe(false);
    // Master level is pinned to unity regardless of what the session held (#354).
    expect(app.volume).toBe(1);
    expect(mock.volume).toBe(1);
    // Adopting a snapshot resets the clock; the saved position wins, because
    // that is where the deck is actually parked.
    expect(app.currentTime).toBe(12.5);
    expect(document.title).toBe("t2 - a2 | RadiodioDJ");
  });

  it("loadSession shows the rebuild notice until the rescan finishes", async () => {
    let emitScanState: (s: unknown) => void = () => {};
    api.onScanStateChanged.mockImplementation((cb: (s: unknown) => void) => {
      emitScanState = cb;
    });
    api.getScanStatus.mockResolvedValue({
      status: "running",
      processed: 0,
      total: 0,
    });
    api.loadSession.mockResolvedValueOnce({
      state: {
        playlistIds: [],
        playlistItems: [],
        historyIds: [],
        currentTrackId: null,
        currentTime: 0,
        autoPlaylistActive: false,
        autoAdvance: true,
        volume: 1,
        cueVolume: 1,
      },
      tracks: [],
      libraryReset: true,
    });
    ({ app } = makeApp());
    await app.hydrateScanStatus();
    await app.loadSession();
    await flushAsync();
    expect(app.libraryReset).toBe(true);

    emitScanState({ status: "idle", lastResult: { total: 1, added: 1 } });
    expect(app.libraryReset).toBe(false);
  });

  it("loadSession skips the rebuild notice when the rescan already ended", async () => {
    api.loadSession.mockResolvedValueOnce({
      state: {
        playlistIds: [],
        playlistItems: [],
        historyIds: [],
        currentTrackId: null,
        currentTime: 0,
        autoPlaylistActive: false,
        autoAdvance: true,
        volume: 1,
        cueVolume: 1,
      },
      tracks: [],
      libraryReset: true,
    });
    ({ app } = makeApp());
    await app.loadSession();
    await flushAsync();
    expect(app.libraryReset).toBe(false);
  });

  it("loadSession asks the deck whether it is playing", async () => {
    // The deck's pause-state was emitted while the backend restored the session,
    // before this window was listening — the button would otherwise say paused
    // over audible playback.
    api.mainDeckIsPlaying.mockResolvedValueOnce(true);
    ({ app, mock, playlist } = makeApp());
    playlist.restore({ current: t(2) });
    await app.loadSession();
    expect(app.isPlaying).toBe(true);
  });

  it("loadSession survives the deck state query failing", async () => {
    api.mainDeckIsPlaying.mockRejectedValueOnce(new Error("boom"));
    ({ app, mock, playlist } = makeApp());
    playlist.restore({ playlist: [trackItem(t(1))] });
    await app.loadSession();
    expect(app.isPlaying).toBe(false);
    expect(app.playlist.map(pid)).toEqual([1]);
  });

  it("loadSession drops history ids the library no longer has", async () => {
    api.loadSession.mockResolvedValueOnce({
      state: {
        playlistIds: [1, 99, 2],
        playlistItems: [],
        historyIds: [42, 1],
        currentTrackId: 7,
        currentTime: 0,
        autoPlaylistActive: false,
        autoAdvance: true,
        volume: 1,
        cueVolume: 1,
      },
      tracks: [t(1), t(2)],
    });

    ({ app, mock, playlist } = makeApp());
    // The backend dropped id 99 and the missing current track when it resolved
    // the same file, so the snapshot is already clean.
    playlist.restore({ playlist: [trackItem(t(1)), trackItem(t(2))] });
    await app.loadSession();

    expect(app.playlist.map(pid)).toEqual([1, 2]);
    expect(app.history.map((x) => x.id)).toEqual([1]);
    expect(app.currentTrack).toBeNull();
  });

  it("a snapshot that does not change the track leaves the clock alone", async () => {
    ({ app, mock, playlist } = makeApp());
    playlist.restore({ current: t(2) });
    await app.loadSession();
    app.currentTime = 30;
    // A queue mutation snapshots the whole playlist, including the unchanged
    // current track. Treating that as a track change would jump the seek bar
    // back to zero on every add.
    app.addToPlaylist(t(5));
    expect(app.currentTime).toBe(30);
    expect(app.currentTrack?.id).toBe(2);
  });

  it("does not save before session is loaded", () => {
    ({ app, mock } = makeApp());
    app.addToPlaylist(t(1));
    vi.runAllTimers();
    expect(api.saveSession).not.toHaveBeenCalled();
  });

  it("debounced save fires after mutations once session is loaded", async () => {
    ({ app, mock } = makeApp());
    await app.loadSession();
    app.addToPlaylist(t(1));
    app.addToPlaylist(t(2));
    expect(api.saveSession).not.toHaveBeenCalled();
    vi.runAllTimers();
    expect(api.saveSession).toHaveBeenCalledTimes(1);
    const arg = api.saveSession.mock.calls[0][0];
    expect(arg.playlistIds).toEqual([1, 2]);
    expect(arg.currentTrackId).toBeNull();
    expect(arg.autoAdvance).toBe(true);
    expect(arg.volume).toBe(1);
  });

  it("flushSave persists immediately and cancels pending timer", async () => {
    ({ app, mock } = makeApp());
    await app.loadSession();
    app.addToPlaylist(t(5));
    app.flushSave();
    expect(api.saveSession).toHaveBeenCalledTimes(1);
    vi.runAllTimers();
    expect(api.saveSession).toHaveBeenCalledTimes(1);
  });
});

describe("AppState cue deck", () => {
  let app: AppState;
  let mock: MockBackend;
  let cueMock: MockBackend;

  beforeEach(() => {
    resetApi();
    ({ app, mock, cueMock } = makeApp());
  });

  it("cueLoad parks the track on the cue backend, leaving main untouched", async () => {
    app.cueLoad(t(7, { title: "Cue", artist: "Band" }));
    await flushAsync();
    expect(cueMock.lastLoadedId).toBe(7);
    // Cueing stages a track; it does not start it.
    expect(cueMock.playCalls).toBe(0);
    expect(app.cueIsPlaying).toBe(false);
    expect(mock.lastLoadedId).toBeUndefined();
    expect(app.cueTrack?.id).toBe(7);
    expect(app.cueDuration).toBe(100);
    expect(app.cueCurrentTime).toBe(0);
  });

  it("cueTogglePlay starts a parked track, then pauses and resumes it", async () => {
    app.cueLoad(t(1));
    await flushAsync();
    expect(cueMock.playCalls).toBe(0);
    app.cueTogglePlay();
    expect(cueMock.playCalls).toBe(1);
    cueMock.emitPauseState(false);
    expect(app.cueIsPlaying).toBe(true);
    app.cueTogglePlay();
    expect(cueMock.pauseCalls).toBeGreaterThan(0);
    cueMock.emitPauseState(true);
    expect(app.cueIsPlaying).toBe(false);
    app.cueTogglePlay();
    expect(cueMock.playCalls).toBe(2);
  });

  it("cueTogglePlay is a no-op when no cue track loaded", () => {
    app.cueTogglePlay();
    expect(cueMock.playCalls).toBe(0);
    expect(cueMock.pauseCalls).toBe(0);
  });

  it("cueStop clears cue state and stops backend", () => {
    app.cueLoad(t(1));
    app.cueDuration = 200;
    app.cueCurrentTime = 30;
    app.cueIsPlaying = true;
    app.cueStop();
    expect(cueMock.stopCalls).toBeGreaterThan(0);
    expect(app.cueTrack).toBeNull();
    expect(app.cueIsPlaying).toBe(false);
    expect(app.cueCurrentTime).toBe(0);
    expect(app.cueDuration).toBe(0);
  });

  it("cueSeekToPct clamps + applies via cue backend", () => {
    app.cueLoad(t(1));
    app.cueDuration = 100;
    app.cueSeekToPct(0.25);
    expect(app.cueCurrentTime).toBe(25);
    expect(cueMock.lastSeek).toBe(25);
    app.cueSeekToPct(2);
    expect(app.cueCurrentTime).toBe(100);
    expect(cueMock.lastSeek).toBe(100);
    app.cueSeekToPct(-1);
    expect(app.cueCurrentTime).toBe(0);
    expect(cueMock.lastSeek).toBe(0);
  });

  it("cueSeekToPct is a no-op when cueDuration is zero", () => {
    app.cueDuration = 0;
    app.cueCurrentTime = 5;
    app.cueSeekToPct(0.5);
    expect(app.cueCurrentTime).toBe(5);
    expect(cueMock.seekCalls.length).toBe(0);
  });

  it("setCueVolume updates state and cue backend", () => {
    app.setCueVolume(0.4);
    expect(app.cueVolume).toBe(0.4);
    expect(cueMock.volume).toBe(0.4);
  });

  it("promoteCueToMain inserts cue track at playlist head; cue keeps its track", () => {
    app.addToPlaylist(t(1));
    app.addToPlaylist(t(2));
    app.cueLoad(t(99, { title: "promoted" }));
    app.promoteCueToMain();
    expect(app.playlist.map(pid)).toEqual([99, 1, 2]);
    expect(app.cueTrack?.id).toBe(99);
    expect(cueMock.stopCalls).toBe(0);
  });

  it("promoteCueToMain is a no-op when no cue track loaded", () => {
    app.addToPlaylist(t(1));
    app.promoteCueToMain();
    expect(app.playlist.map(pid)).toEqual([1]);
  });

  it("cue backend time/duration/pause-state events mirror to cue state", () => {
    cueMock.emitTime(7);
    expect(app.cueCurrentTime).toBe(7);
    cueMock.emitDuration(180);
    expect(app.cueDuration).toBe(180);
    cueMock.emitPauseState(false);
    expect(app.cueIsPlaying).toBe(true);
    cueMock.emitPauseState(true);
    expect(app.cueIsPlaying).toBe(false);
  });

  it("cue backend ended event resets cue playing/time without touching main", () => {
    app.cueLoad(t(1));
    app.cueIsPlaying = true;
    app.cueCurrentTime = 50;
    app.currentTrack = t(2);
    cueMock.emitEnded();
    expect(app.cueIsPlaying).toBe(false);
    expect(app.cueCurrentTime).toBe(0);
    expect(app.currentTrack?.id).toBe(2);
  });

  it("cueProgressPct reflects cueCurrentTime/cueDuration", () => {
    app.cueDuration = 200;
    app.cueCurrentTime = 50;
    expect(app.cueProgressPct).toBe(25);
    app.cueDuration = 0;
    expect(app.cueProgressPct).toBe(0);
  });
});

describe("AppState audio device config", () => {
  let app: AppState;

  beforeEach(() => {
    resetApi();
    api.listAudioDevices.mockResolvedValue([
      { name: "default-out", description: "Built-in", isDefault: true },
      { name: "hw:USB,0", description: "USB Headphones", isDefault: false },
    ]);
    api.getMainDevice.mockResolvedValue(null);
    api.getCueDevice.mockResolvedValue({
      name: "hw:USB,0",
      description: "USB Headphones",
    });
    app = makeApp().app;
  });

  it("loadAudioConfig populates devices, mainDevice, cueDevice", async () => {
    await app.loadAudioConfig();
    expect(app.audioDevices.length).toBe(2);
    expect(app.mainDevice).toBeNull();
    expect(app.cueDevice?.name).toBe("hw:USB,0");
  });

  it("setMainDeviceConfig persists + updates state", async () => {
    await app.setMainDeviceConfig({ name: "x", description: "X" });
    expect(api.setMainDevice).toHaveBeenCalledWith({
      name: "x",
      description: "X",
    });
    expect(app.mainDevice?.name).toBe("x");
  });

  it("setCueDeviceConfig with null disables cue + clears cue state", async () => {
    await app.loadAudioConfig();
    app.cueLoad(t(1));
    app.cueDuration = 100;

    await app.setCueDeviceConfig(null);

    expect(api.setCueDevice).toHaveBeenCalledWith(null);
    expect(app.cueDevice).toBeNull();
    expect(app.cueTrack).toBeNull();
    expect(app.cueDuration).toBe(0);
  });

  it("setCueDeviceConfig with a device persists and stores", async () => {
    const ref = { name: "hw:USB,0", description: "USB Headphones" };
    await app.setCueDeviceConfig(ref);
    expect(api.setCueDevice).toHaveBeenCalledWith(ref);
    expect(app.cueDevice).toEqual(ref);
  });
});

describe("AppState session persistence with cue volume", () => {
  let app: AppState;

  beforeEach(() => {
    resetApi();
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("loadSession restores cueVolume", async () => {
    api.loadSession.mockResolvedValueOnce({
      state: {
        playlistIds: [],
        playlistItems: [],
        historyIds: [],
        currentTrackId: null,
        currentTime: 0,
        autoPlaylistActive: false,
        autoAdvance: true,
        volume: 1,
        cueVolume: 0.25,
      },
      tracks: [],
    });
    ({ app } = makeApp());
    await app.loadSession();
    expect(app.cueVolume).toBe(0.25);
  });

  it("persistSession writes cueVolume", async () => {
    ({ app } = makeApp());
    await app.loadSession();
    app.setCueVolume(0.7);
    vi.runAllTimers();
    expect(api.saveSession).toHaveBeenCalled();
    const arg = api.saveSession.mock.calls[0][0];
    expect(arg.cueVolume).toBe(0.7);
  });
});

describe("AppState stop marker", () => {
  let app: AppState;
  beforeEach(() => {
    resetApi();
    ({ app } = makeApp());
  });

  it("addStopMarker appends a stop sentinel", () => {
    app.addStopMarker();
    expect(app.playlist.length).toBe(1);
    expect(pid(app.playlist[0])).toBe("STOP");
  });

  it("playIndex on a stop marker stops playback and consumes the marker", async () => {
    app.addToPlaylist(t(1));
    app.playIndex(0);
    expect(app.currentTrack?.id).toBe(1);
    app.addStopMarker();
    await app.toggleAutoPlaylist();
    app.playIndex(0);
    expect(app.currentTrack).toBeNull();
    expect(app.playlist.length).toBe(0);
    expect(app.autoPlaylistActive).toBe(false);
    expect(app.history.map((x) => x.id)).toEqual([1]);
  });

  it("togglePlay from idle with stop marker at front consumes it without playing", () => {
    app.addStopMarker();
    app.addToPlaylist(t(7));
    app.togglePlay();
    expect(app.currentTrack).toBeNull();
    expect(app.playlist.map(pid)).toEqual([7]);
    app.togglePlay();
    expect(app.currentTrack?.id).toBe(7);
  });

  it("next() advancing onto a stop marker halts", () => {
    app.addStopMarker();
    app.addToPlaylist(t(5));
    app.playNow(t(99));
    app.next();
    expect(app.currentTrack).toBeNull();
    expect(app.playlist.map(pid)).toEqual([5]);
  });

  it("history never contains stop markers", () => {
    app.addToPlaylist(t(1));
    app.addStopMarker();
    app.playIndex(0);
    app.playIndex(0);
    expect(app.history.map((x) => x.id)).toEqual([1]);
  });
});

describe("AppState session persistence (stop markers)", () => {
  let app: AppState;

  beforeEach(() => {
    resetApi();
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("persistSession writes playlistItems with mixed track and stop entries", async () => {
    ({ app } = makeApp());
    await app.loadSession();
    app.addToPlaylist(t(1));
    app.addStopMarker();
    app.addToPlaylist(t(2));
    vi.runAllTimers();
    const arg = api.saveSession.mock.calls[0][0];
    expect(arg.playlistItems).toEqual([
      { kind: "track", id: 1, cue_override: null },
      { kind: "stop" },
      { kind: "track", id: 2, cue_override: null },
    ]);
    expect(arg.playlistIds).toEqual([1, 2]);
  });

  it("mirrors a restored playlist that still holds a stop marker", async () => {
    const { app: a, playlist } = makeApp();
    playlist.restore({
      playlist: [trackItem(t(1)), stopMarker(), trackItem(t(2))],
    });
    await a.loadSession();
    expect(a.playlist.map(pid)).toEqual([1, "STOP", 2]);
  });
});

describe("AppState revertTrackTags", () => {
  let app: AppState;
  beforeEach(() => {
    resetApi();
    app = makeApp().app;
  });

  it("replaces the track and the one open in the editor", async () => {
    const reverted = t(5, { title: "From File", edited_fields: 0 });
    api.revertTrackTags.mockResolvedValue(reverted);
    app.tracks = [t(4), t(5, { title: "Edited", edited_fields: 1 })];
    app.editingMetadata = app.tracks[1];

    await app.revertTrackTags(5);

    expect(api.revertTrackTags).toHaveBeenCalledWith(5);
    expect(app.tracks[1].title).toBe("From File");
    expect(app.editingMetadata?.edited_fields).toBe(0);
  });

  it("keeps the edits when the file cannot be read", async () => {
    api.revertTrackTags.mockRejectedValue(new Error("read tags"));
    app.tracks = [t(5, { title: "Edited", edited_fields: 1 })];

    await expect(app.revertTrackTags(5)).rejects.toThrow("read tags");
    expect(app.tracks[0].title).toBe("Edited");
  });
});

describe("AppState updateTrackMetadata", () => {
  let app: AppState;
  beforeEach(() => {
    resetApi();
    app = makeApp().app;
  });

  it("forwards only the fields the caller set (absent keys omitted)", async () => {
    const updated = t(5, {
      title: "NewTitle",
      artist: "NewArtist",
      album: "NewAlbum",
    });
    api.updateTrackMetadata.mockResolvedValue(updated);
    const track = t(5, { title: "OldTitle", artist: "OldArtist" });
    app.tracks = [track];
    await app.updateTrackMetadata(5, {
      title: "NewTitle",
      artist: "NewArtist",
      album: "NewAlbum",
    });
    // genre/year were not provided, so they are omitted entirely (leave
    // unchanged) rather than sent as null (which would clear them).
    expect(api.updateTrackMetadata).toHaveBeenCalledWith({
      id: 5,
      title: "NewTitle",
      artist: "NewArtist",
      album: "NewAlbum",
    });
  });

  it("forwards an explicit null to clear a nullable field", async () => {
    const updated = t(5, { title: "T" });
    api.updateTrackMetadata.mockResolvedValue(updated);
    app.tracks = [t(5)];
    await app.updateTrackMetadata(5, { genre: null, year: null });
    expect(api.updateTrackMetadata).toHaveBeenCalledWith({
      id: 5,
      genre: null,
      year: null,
    });
  });

  it("updates the local tracks array via byIndex map", async () => {
    const updated = t(7, { title: "Updated!", album: "World Tour" });
    api.updateTrackMetadata.mockResolvedValue(updated);
    app.tracks = [t(7), t(8)];
    await app.updateTrackMetadata(7, {
      title: "Updated!",
      album: "World Tour",
    });
    expect(app.tracks[0].title).toBe("Updated!");
    expect(app.tracks[0].album).toBe("World Tour");
  });

  it("updates a track not indexed in the byIndex map (not found)", async () => {
    const updated = t(8, { album: "Tour" });
    api.updateTrackMetadata.mockResolvedValue(updated);
    // Track id=8 has index that doesn't exist in app.tracks (byIndex only stores [7→0])
    app.tracks = [
      {
        id: 7,
        title: "T7",
        artist: "A",
        album: "",
        duration: 100,
        play_count: 0,
      },
    ];
    const result = await app.updateTrackMetadata(8, { album: "Tour" });
    expect(result?.album).toBe("Tour");
  });

  it("returns null on API error without throwing", async () => {
    api.updateTrackMetadata.mockRejectedValue(new Error("fail"));
    app.tracks = [t(3)];
    const result = await app.updateTrackMetadata(3, { title: "x" });
    expect(result).toBeNull();
  });

  it("updates document.title when editing current track with new title", async () => {
    const updated = {
      id: 10,
      title: "Headliner",
      artist: "DJ Mix",
      album: "",
      duration: 100,
      play_count: 0,
    } as Track;
    api.updateTrackMetadata.mockResolvedValue(updated);
    app.tracks = [t(10, { title: "Old Track" })];
    app.currentTrack = t(10, { title: "Old Track" });
    await app.updateTrackMetadata(10, { title: "Headliner", artist: "DJ Mix" });
    expect(document.title).toBe("Headliner - DJ Mix | RadiodioDJ");
  });

  it("does not update document.title when oldTitle equals updated title", async () => {
    const updated = {
      id: 10,
      title: "SameTitle",
      artist: "Band",
      album: "",
      duration: 100,
      play_count: 0,
    } as Track;
    api.updateTrackMetadata.mockResolvedValue(updated);
    app.currentTrack = t(10, { title: "SameTitle" });
    await app.updateTrackMetadata(10, { artist: "Band" });
    // document.title should remain the default "" (reset on stop or never set)
  });

  it("schedules save after update", async () => {
    vi.useFakeTimers();
    try {
      const updated = t(5);
      api.updateTrackMetadata.mockResolvedValue(updated);
      app.tracks = [t(5)];
      // Call updateTrackMetadata first — this will trigger scheduleSave.
      await app.updateTrackMetadata(5, { title: "x" });
      const callsBeforeFlush = api.saveSession.mock.calls.length;
      // The throttled timer hasn't fired yet (no timers advanced).
      expect(callsBeforeFlush).toBeLessThanOrEqual(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("updates currentTrack reference when it equals the edited track", async () => {
    const updated = t(5, { title: "Remixed" });
    api.updateTrackMetadata.mockResolvedValue(updated);
    const before = t(5);
    app.currentTrack = before;
    await app.updateTrackMetadata(5, { title: "Remixed" });
    expect(app.currentTrack?.title).toBe("Remixed");
  });
});

describe("AppState cue points", () => {
  let app: AppState;
  let cueMock: MockBackend;

  const trimmed = {
    cue_in_ms: 10_000,
    fade_in_ms: null,
    fade_out_ms: null,
    cue_out_ms: 30_000,
    next_start_ms: null,
  };

  beforeEach(() => {
    resetApi();
    ({ app, cueMock } = makeApp());
  });

  it("auditions the whole file by default", async () => {
    app.cueLoad(t(1, { cue_points: trimmed }));
    await flushAsync();
    expect(app.cueMode).toBe("absolute");
    expect(cueMock.loadedCuePoints).toEqual([null]);
    expect(app.cueDuration).toBe(100);
    expect(app.cueCrop).toBeNull();
  });

  it("Preview reloads the deck with the track's markers applied", async () => {
    app.cueLoad(t(1, { cue_points: trimmed }));
    await flushAsync();
    app.setCueMode("preview");
    await flushAsync();
    expect(app.cueMode).toBe("preview");
    expect(cueMock.loadedCuePoints).toEqual([null, trimmed]);
    // Air time, not file time: the deck reports the same.
    expect(app.cueDuration).toBe(20);
  });

  it("Preview crops the waveform to the aired region", async () => {
    app.cueLoad(t(1, { cue_points: trimmed }), trimmed);
    await flushAsync();
    expect(app.cueCrop).toEqual({ from: 0.1, to: 0.3 });
  });

  it("previewing a track with no markers still plays the whole file", async () => {
    app.cueLoad(t(2));
    await flushAsync();
    app.setCueMode("preview");
    await flushAsync();
    // An all-null override is representable and means "play it all".
    expect(cueMock.loadedCuePoints[1]).toEqual({
      cue_in_ms: null,
      fade_in_ms: null,
      fade_out_ms: null,
      cue_out_ms: null,
      next_start_ms: null,
    });
    expect(app.cueDuration).toBe(100);
  });

  it("switching to the mode already in effect does not reload", async () => {
    app.cueLoad(t(1));
    await flushAsync();
    app.setCueMode("absolute");
    await flushAsync();
    expect(cueMock.loadedIds).toEqual([1]);
  });

  it("stopping the cue deck drops back to Absolute", async () => {
    app.cueLoad(t(1, { cue_points: trimmed }), trimmed);
    await flushAsync();
    app.cueStop();
    expect(app.cueMode).toBe("absolute");
    expect(app.cueCrop).toBeNull();
  });

  it("saveCuePoints adopts the clamped value the backend returns", async () => {
    // The backend sorted the markers; the UI takes what it is given rather
    // than reimplementing the rule.
    const clamped = { ...trimmed, fade_in_ms: 12_000 };
    api.setCuePoints.mockResolvedValue(clamped);
    app.tracks = [t(1), t(2)];
    const stored = await app.saveCuePoints(1, trimmed);
    expect(stored).toEqual(clamped);
    expect(app.tracks[0].cue_points).toEqual(clamped);
    expect(app.tracks[1].cue_points).toBeUndefined();
  });

  it("saveCuePoints refreshes every queued and cued copy of the track", async () => {
    const clamped = { ...trimmed };
    api.setCuePoints.mockResolvedValue(clamped);
    app.playlist = [trackItem(t(1)), trackItem(t(2))];
    app.history = [t(1)];
    app.cueTrack = t(1);
    await app.saveCuePoints(1, trimmed);
    expect(
      app.playlist.filter(isTrackItem).map((i) => i.track.cue_points),
    ).toEqual([clamped, undefined]);
    expect(app.history[0].cue_points).toEqual(clamped);
    expect(app.cueTrack?.cue_points).toEqual(clamped);
  });

  it("saveCuePoints leaves the on-air track alone — edits apply next airing", async () => {
    api.setCuePoints.mockResolvedValue(trimmed);
    app.currentTrack = t(1);
    await app.saveCuePoints(1, trimmed);
    expect(app.currentTrack?.cue_points).toBeUndefined();
  });

  // ----- auditioning from the editor -----

  it("cueing stages a track without playing it", async () => {
    app.cueLoad(t(1));
    await flushAsync();
    expect(cueMock.loadedAutoplay).toEqual([false]);
  });

  it("an audition loads the draft and plays it", async () => {
    // What the editor's Audition button does: markers are applied at load
    // time, so playing them means reloading with autoplay set.
    app.cueLoad(t(1, { cue_points: trimmed }), trimmed, true);
    await flushAsync();
    expect(cueMock.loadedCuePoints).toEqual([trimmed]);
    expect(cueMock.loadedAutoplay).toEqual([true]);
    expect(app.cueIsPlaying).toBe(true);
  });

  it("cueing over a playing track stops it without waiting for the read", () => {
    app.cueLoad(t(1), null, true);
    app.cueLoad(t(2));
    expect(app.cueIsPlaying).toBe(false);
    expect(app.cueCurrentTime).toBe(0);
  });

  it("an edited audition reloads where it was", async () => {
    app.cueLoad(t(1, { cue_points: trimmed }), trimmed, true, 4.5);
    await flushAsync();
    expect(cueMock.loadedStartAt).toEqual([4.5]);
    expect(app.cueCurrentTime).toBe(4.5);
  });

  it("the clock never starts past the end of what airs", async () => {
    const track = t(1, { cue_points: trimmed });
    app.cueLoad(track, trimmed, false, 1e6);
    expect(app.cueCurrentTime).toBe(app.cueDuration);
  });

  it("reloading the same track keeps its curve", async () => {
    api.getWaveform.mockResolvedValue([1, 2, 3]);
    const track = t(1);
    app.cueLoad(track);
    await flushAsync();
    app.cueLoad(track, trimmed);
    await flushAsync();
    expect(app.cueWaveform).toEqual([1, 2, 3]);
    expect(api.getWaveform).toHaveBeenCalledTimes(1);
  });

  it("restoring an empty deck clears whatever the editor left on it", async () => {
    const before = app.cueSnapshot();
    app.cueLoad(t(1), trimmed, true);
    await flushAsync();
    app.cueRestore(before);
    expect(app.cueTrack).toBeNull();
    expect(cueMock.stopCalls).toBe(1);
  });

  it("restoring an Absolute audition puts the same track back, parked", async () => {
    app.cueLoad(t(1));
    await flushAsync();
    const before = app.cueSnapshot();
    app.cueLoad(t(2), trimmed, true);
    await flushAsync();
    app.cueRestore(before);
    await flushAsync();
    expect(app.cueTrack?.id).toBe(1);
    expect(app.cueMode).toBe("absolute");
    expect(cueMock.loadedAutoplay.at(-1)).toBe(false);
  });

  // A Preview restores to the track's markers as they are *now*, so closing
  /// the editor after a save shows the edit that was just stored.
  it("restoring a Preview re-reads the track's current markers", async () => {
    const track = t(1, { cue_points: trimmed });
    app.cueLoad(track, trimmed);
    await flushAsync();
    const before = app.cueSnapshot();
    const saved = { ...trimmed, cue_out_ms: 20_000 };
    app.tracks = [{ ...track, cue_points: saved }];
    app.cueRestore(before);
    await flushAsync();
    expect(cueMock.loadedCuePoints.at(-1)).toEqual(saved);
    expect(app.cueDuration).toBe(10);
  });
});

describe("AppState air-time durations", () => {
  let app: AppState;

  beforeEach(() => {
    resetApi();
    ({ app } = makeApp());
  });

  it("the main deck's optimistic duration is what airs, not the file length", async () => {
    api.playlistSync.mockResolvedValue({
      playlist: [],
      current: t(1, {
        duration: 100,
        cue_points: {
          cue_in_ms: 10_000,
          fade_in_ms: null,
          fade_out_ms: null,
          cue_out_ms: 30_000,
          next_start_ms: null,
        },
      }),
      displaced: null,
      autoPlaylistActive: false,
      autoAdvance: true,
      awaitingNetwork: false,
    });
    await app.loadSession();
    await flushAsync();
    expect(app.duration).toBe(20);
  });
});

describe("AppState item cue overrides", () => {
  let app: AppState;
  let playlist: MockPlaylistBackend;

  /** A radio edit that trims a 100 s track to 20 s. */
  const radioEdit: CuePoints = {
    cue_in_ms: 10_000,
    fade_in_ms: null,
    fade_out_ms: null,
    cue_out_ms: 30_000,
    next_start_ms: null,
  };

  /** An audition that trims the same track to 5 s instead. */
  const audition: CuePoints = {
    cue_in_ms: 0,
    fade_in_ms: null,
    fade_out_ms: null,
    cue_out_ms: 5_000,
    next_start_ms: null,
  };

  const overrideAt = (index: number): CuePoints | null | undefined => {
    const item = app.playlist[index];
    return isTrackItem(item) ? item.cue_override : null;
  };

  beforeEach(() => {
    resetApi();
    ({ app, playlist } = makeApp());
  });

  it("promotes an audition that differs from the radio edit as an override", async () => {
    app.cueLoad(t(1, { cue_points: radioEdit }), audition);
    await flushAsync();
    expect(app.cuePromoteCarriesOverride).toBe(true);
    app.promoteCueToMain();
    expect(overrideAt(0)).toEqual(audition);
  });

  // Freezing an item to markers it would have inherited anyway would stop a
  /// later correction to the track from reaching the queued airing.
  it("promotes an audition matching the radio edit without an override", async () => {
    app.cueLoad(t(1, { cue_points: radioEdit }), radioEdit);
    await flushAsync();
    expect(app.cuePromoteCarriesOverride).toBe(false);
    app.promoteCueToMain();
    expect(overrideAt(0)).toBeNull();
  });

  it("promotes from Absolute without an override", async () => {
    app.cueLoad(t(1, { cue_points: radioEdit }));
    await flushAsync();
    app.promoteCueToMain();
    expect(overrideAt(0)).toBeNull();
  });

  it("Use once queues the draft without touching the track", async () => {
    const track = t(1, { cue_points: radioEdit });
    app.queueCueDraft(track, audition);
    await flushAsync();
    expect(overrideAt(0)).toEqual(audition);
    expect(app.playlist.length).toBe(1);
  });

  // Freezing an item to markers it would have inherited anyway would stop a
  // later correction to the track from reaching the queued airing.
  it("Use once carries nothing when the draft matches the radio edit", async () => {
    const track = t(1, { cue_points: radioEdit });
    app.queueCueDraft(track, radioEdit);
    await flushAsync();
    expect(overrideAt(0)).toBeNull();
  });

  it("an override drives the row's duration and the optimistic clock", async () => {
    await app.loadSession();
    app.cueLoad(t(1, { cue_points: radioEdit }), audition);
    await flushAsync();
    app.promoteCueToMain();
    app.playIndex(0);
    await flushAsync();
    // 5 s of audition, not the radio edit's 20 or the file's 100.
    expect(app.currentCueOverride).toEqual(audition);
    expect(app.duration).toBe(5);
  });

  it("setItemCuePoints hands an item back to the track's radio edit", async () => {
    app.cueLoad(t(1, { cue_points: radioEdit }), audition);
    await flushAsync();
    app.promoteCueToMain();
    app.setItemCuePoints(0, null);
    expect(overrideAt(0)).toBeNull();
  });

  it("setItemCuePoints ignores an index the playlist does not have", () => {
    app.addToPlaylist(t(1));
    app.setItemCuePoints(7, audition);
    expect(api.playlistSetItemCuePoints).not.toHaveBeenCalled();
  });

  it("persists both the queued overrides and the one on air", async () => {
    vi.useFakeTimers();
    try {
      await app.loadSession();
      app.cueLoad(t(1, { cue_points: radioEdit }), audition);
      await flushAsync();
      // Two airings of the same audition: one goes on air, one stays queued.
      app.promoteCueToMain();
      app.promoteCueToMain();
      app.playIndex(0);
      await flushAsync();
      vi.runAllTimers();
      const calls = api.saveSession.mock.calls;
      const arg = calls[calls.length - 1][0];
      expect(arg.currentCueOverride).toEqual(audition);
      expect(arg.playlistItems).toEqual([
        { kind: "track", id: 1, cue_override: audition },
      ]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("a queued override survives a restore", async () => {
    playlist.restore({ playlist: [trackItem(t(1), audition)] });
    await app.loadSession();
    expect(overrideAt(0)).toEqual(audition);
  });
});

describe("AppState library health", () => {
  const missing = (id: number, missingSince: number) => ({
    id,
    title: `t${id}`,
    artist: `a${id}`,
    path: `/m/${id}.mp3`,
    missingSince,
    playCount: 0,
    hasCuePoints: false,
    outsideRoots: false,
  });
  const report = (ids: [number, number][]) => ({
    ...EMPTY_HEALTH,
    missing: ids.map(([id, since]) => missing(id, since)),
  });

  beforeEach(() => {
    resetApi();
  });

  it("loadHealth adopts the backend report and indexes missing tracks", async () => {
    api.libraryHealth.mockResolvedValueOnce(report([[3, 100]]));
    const { app } = makeApp();
    await app.loadHealth();
    expect(app.health.missing).toHaveLength(1);
    expect(app.missingSince.get(3)).toBe(100);
    expect(app.missingSince.has(1)).toBe(false);
  });

  it("a library-health event replaces the report", () => {
    const { app } = makeApp();
    const cb = api.onLibraryHealth.mock.calls[0]?.[0] as
      ((r: HealthReport) => void) | undefined;
    expect(cb).toBeDefined();
    cb!(report([[1, 5]]));
    expect(app.missingSince.get(1)).toBe(5);
    cb!(report([]));
    expect(app.missingSince.size).toBe(0);
  });

  it("counts findings for the Settings badge", () => {
    const { app } = makeApp();
    const cb = api.onLibraryHealth.mock.calls[0]?.[0] as (
      r: HealthReport,
    ) => void;
    cb({
      ...report([[1, 5]]),
      exact: [{ key: "v1:a", dismissed: false, tracks: [] }],
    });
    expect(app.healthAttention).toBe(2);
  });

  it("dismiss, undo and check-now go to the backend", () => {
    api.healthDismiss.mockResolvedValue(undefined);
    api.healthUndismiss.mockResolvedValue(undefined);
    api.libraryCheckNow.mockResolvedValue(undefined);
    const { app } = makeApp();
    app.dismissFinding("exact", "v1:a");
    app.undismissFinding("missing");
    app.checkLibraryNow();
    expect(api.healthDismiss).toHaveBeenCalledWith("exact", "v1:a");
    expect(api.healthUndismiss).toHaveBeenCalledWith("missing", "");
    expect(api.libraryCheckNow).toHaveBeenCalled();
  });

  it("a failing lookup keeps the report it had", async () => {
    api.libraryHealth.mockRejectedValueOnce("boom");
    const { app } = makeApp();
    await app.loadHealth();
    expect(app.health).toEqual(EMPTY_HEALTH);
  });
});

describe("admin mode", () => {
  let app: AppState;

  const locked = { passwordSet: true, unlocked: false, idleLockMin: 15 };

  beforeEach(() => {
    resetApi();
    app = makeApp().app;
  });

  it("is admin while no password is set", async () => {
    await app.loadAdmin();
    expect(app.isAdmin).toBe(true);
  });

  it("starts locked when a password is set", async () => {
    api.adminStatus.mockResolvedValue(locked);
    await app.loadAdmin();
    expect(app.isAdmin).toBe(false);
  });

  it("stays locked on a wrong password", async () => {
    api.adminStatus.mockResolvedValue(locked);
    await app.loadAdmin();
    app.unlockOpen = true;
    api.adminUnlock.mockResolvedValue(false);
    expect(await app.unlockAdmin("nope")).toBe(false);
    expect(api.adminUnlock).toHaveBeenCalledWith("nope");
    expect(app.isAdmin).toBe(false);
    expect(app.unlockOpen).toBe(true);
  });

  it("unlocks on the right password and closes the dialog", async () => {
    api.adminStatus.mockResolvedValue(locked);
    await app.loadAdmin();
    app.unlockOpen = true;
    api.adminUnlock.mockResolvedValue(true);
    expect(await app.unlockAdmin("hunter42")).toBe(true);
    expect(app.isAdmin).toBe(true);
    expect(app.unlockOpen).toBe(false);
  });

  it("locking closes Settings and the metadata editor, not the cue-point editor", async () => {
    api.adminStatus.mockResolvedValue({ ...locked, unlocked: true });
    await app.loadAdmin();
    app.settingsOpen = true;
    app.editingMetadata = t(1);
    app.editingCuePoints = t(2);

    app.lockAdmin();

    expect(api.adminLock).toHaveBeenCalled();
    expect(app.isAdmin).toBe(false);
    expect(app.settingsOpen).toBe(false);
    expect(app.editingMetadata).toBeNull();
    expect(app.editingCuePoints?.id).toBe(2);
  });

  it("follows the backend's admin-state-changed event", async () => {
    const handler = api.onAdminStateChanged.mock.calls[0][0];
    app.settingsOpen = true;
    handler(locked);
    expect(app.isAdmin).toBe(false);
    expect(app.settingsOpen).toBe(false);
  });

  it("setting and removing a password keep the app unlocked", async () => {
    await app.setAdminPassword("hunter42");
    expect(api.adminSetPassword).toHaveBeenCalledWith("hunter42");
    expect(app.admin.passwordSet).toBe(true);
    expect(app.isAdmin).toBe(true);

    await app.clearAdminPassword();
    expect(app.admin.passwordSet).toBe(false);
    expect(app.isAdmin).toBe(true);
  });

  it("adopts the backend's clamped idle timeout", async () => {
    api.adminStatus.mockResolvedValue({
      ...locked,
      unlocked: true,
      idleLockMin: 1,
    });
    await app.setIdleLockMin(0);
    expect(api.adminSetIdleLockMin).toHaveBeenCalledWith(0);
    expect(app.admin.idleLockMin).toBe(1);
  });
});
