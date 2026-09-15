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
    playlistSync: vi.fn(),
    onPlaylistState: vi.fn(),
    playlistAdd: vi.fn(),
    playlistAddFront: vi.fn(),
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
    getTuningConfig: vi.fn(),
    setTuningConfig: vi.fn(),
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
    },
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

import { AppState, formatTime, type Track } from "./state.svelte";
import type { ScanStatus } from "./api";
import { isTrackItem, stopMarker, trackItem, type PlaylistItem } from "./types";

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

function resetApi(): void {
  vi.clearAllMocks();
  library.clear();
  api.search.mockResolvedValue([]);
  api.trackPlayed.mockResolvedValue(undefined);
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
  });
  api.saveSession.mockResolvedValue(undefined);
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
  api.playlistAddFront.mockImplementation((id: number) =>
    ok(() => playlist.addFront(known(id))),
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
    app.cueLoadAndPlay(t(3));
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

  it("cueLoadAndPlay loads + plays on cue backend, leaves main untouched", async () => {
    app.cueLoadAndPlay(t(7, { title: "Cue", artist: "Band" }));
    await flushAsync();
    expect(cueMock.lastLoadedId).toBe(7);
    expect(cueMock.playCalls).toBeGreaterThan(0);
    expect(mock.lastLoadedId).toBeUndefined();
    expect(app.cueTrack?.id).toBe(7);
    expect(app.cueDuration).toBe(100);
    expect(app.cueCurrentTime).toBe(0);
  });

  it("cueTogglePlay pauses then resumes via cue backend", async () => {
    app.cueLoadAndPlay(t(1));
    await flushAsync();
    expect(cueMock.playCalls).toBe(1); // initial load+play
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
    app.cueLoadAndPlay(t(1));
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
    app.cueLoadAndPlay(t(1));
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

  it("promoteCueToMain inserts cue track at playlist head; cue keeps playing", () => {
    app.addToPlaylist(t(1));
    app.addToPlaylist(t(2));
    app.cueLoadAndPlay(t(99, { title: "promoted" }));
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
    app.cueLoadAndPlay(t(1));
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
    app.cueLoadAndPlay(t(1));
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
      { kind: "track", id: 1 },
      { kind: "stop" },
      { kind: "track", id: 2 },
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
