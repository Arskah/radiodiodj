import type { CuePoints, PlaylistItem, Track } from "./types";
import { isStopMarker, stopMarker, trackItem } from "./types";
import type { PlaylistSnapshot } from "./api";

/**
 * In-memory stand-in for the backend-owned playlist, the way `MockBackend`
 * stands in for the audio deck. It models what the renderer can observe —
 * queue order, what is on air, what was displaced — and emits the same
 * `program:playlist-state` snapshots the real service does, so projection tests
 * exercise the whole loop rather than asserting on a spy.
 *
 * Advancement policy is deliberately absent: outage skip-to-cached, auto-playlist
 * refill sizing and retry backoff live in `playlist::engine` on the Rust side and
 * are specified by that module's tests.
 */
export class MockPlaylistBackend {
  playlist: PlaylistItem[] = [];
  current: Track | null = null;
  currentOverride: CuePoints | null = null;
  autoPlaylistActive = false;
  autoAdvance = true;
  awaitingNetwork = false;

  private listeners = new Set<(snapshot: PlaylistSnapshot) => void>();

  on(listener: (snapshot: PlaylistSnapshot) => void): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  snapshot(displaced: Track | null = null): PlaylistSnapshot {
    return {
      playlist: this.playlist.slice(),
      current: this.current,
      displaced,
      autoPlaylistActive: this.autoPlaylistActive,
      autoAdvance: this.autoAdvance,
      currentOverride: this.currentOverride,
      awaitingNetwork: this.awaitingNetwork,
    };
  }

  /** Seed state the way a backend restoring a session file would, silently. */
  restore(state: Partial<Omit<MockPlaylistBackend, "on" | "snapshot">>): void {
    Object.assign(this, state);
  }

  add(track: Track): void {
    this.playlist.push(trackItem(track));
    this.emit();
  }

  addFront(track: Track, cueOverride: CuePoints | null = null): void {
    this.playlist.unshift(trackItem(track, cueOverride));
    this.emit();
  }

  setItemCuePoints(index: number, cueOverride: CuePoints | null): void {
    const item = this.playlist[index];
    if (item && !isStopMarker(item)) {
      this.playlist[index] = trackItem(item.track, cueOverride);
    }
    this.emit();
  }

  addStopMarker(): void {
    this.playlist.push(stopMarker());
    this.emit();
  }

  remove(index: number): void {
    this.playlist.splice(index, 1);
    this.emit();
  }

  move(from: number, to: number): void {
    if (from === to) return;
    const [item] = this.playlist.splice(from, 1);
    this.playlist.splice(to, 0, item);
    this.emit();
  }

  clear(): void {
    this.playlist.length = 0;
    this.emit();
  }

  playIndex(index: number): void {
    if (index < 0 || index >= this.playlist.length) {
      this.emit();
      return;
    }
    const [item] = this.playlist.splice(index, 1);
    if (isStopMarker(item)) {
      this.stop();
      return;
    }
    this.playTrack(item.track, item.cue_override ?? null);
  }

  playNow(track: Track): void {
    this.playTrack(track);
  }

  next(): void {
    if (this.playlist.length === 0) {
      this.emit();
      return;
    }
    this.playIndex(0);
  }

  /** Steps back without displacing: the caller is walking its own history. */
  prev(track: Track): void {
    if (this.current) {
      this.playlist.unshift(trackItem(this.current, this.currentOverride));
    }
    this.current = track;
    // History stores tracks, not items, so a custom airing replays under the
    // radio edit — the same rule the engine follows.
    this.currentOverride = null;
    this.emit();
  }

  stop(): void {
    const displaced = this.current;
    this.current = null;
    this.currentOverride = null;
    this.autoPlaylistActive = false;
    this.emit(displaced);
  }

  setAutoPlaylist(active: boolean): void {
    this.autoPlaylistActive = active;
    if (active && !this.current && this.playlist.length > 0) {
      this.playIndex(0);
      return;
    }
    this.emit();
  }

  setAutoAdvance(active: boolean): void {
    this.autoAdvance = active;
    this.emit();
  }

  /** Drive the reconnecting projection without a real outage. */
  setAwaitingNetwork(awaiting: boolean): void {
    this.awaitingNetwork = awaiting;
    this.emit();
  }

  private playTrack(track: Track, cueOverride: CuePoints | null = null): void {
    const displaced = this.current;
    this.current = track;
    this.currentOverride = cueOverride;
    this.emit(displaced);
  }

  private emit(displaced: Track | null = null): void {
    const snapshot = this.snapshot(displaced);
    for (const listener of this.listeners) listener(snapshot);
  }
}
