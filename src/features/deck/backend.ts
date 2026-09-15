export type DeckEvent =
  | { type: "time"; seconds: number }
  | { type: "duration"; seconds: number }
  | { type: "pause-state"; paused: boolean }
  | { type: "ended" }
  | { type: "buffering"; buffering: boolean }
  | { type: "cache-state"; ids: number[] }
  | { type: "load-failed"; id: number }
  | { type: "prefetch-failed" }
  | { type: "output-unavailable"; unavailable: boolean }
  | { type: "error"; message: string };

export type DeckEventHandler = (event: DeckEvent) => void;

/**
 * Deck transport. The main deck exposes only this: what is loaded on it is
 * decided by the backend-owned playlist, so the renderer can start, stop and
 * seek what is on air but cannot put a track there behind the playlist's back.
 */
export interface DeckTransport {
  play(): Promise<void>;
  pause(): Promise<void>;
  stop(): Promise<void>;
  seek(seconds: number): Promise<void>;
  setVolume(volume: number): Promise<void>;
  on(handler: DeckEventHandler): () => void;
  dispose(): Promise<void>;
  /** Resolves once the deck's event subscriptions are live. */
  whenReady(): Promise<void>;
}

/** A deck the renderer also loads tracks onto — the cue deck. */
export interface DeckBackend extends DeckTransport {
  load(trackId: number): Promise<void>;
}
