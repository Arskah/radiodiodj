import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { DeckBackend, DeckEvent, DeckEventHandler } from "./backend";
import type { CuePoints } from "../../shared/types";

/**
 * Domain id of a deck. Main deck is on-air; Cue deck is off-air preview.
 * Maps to per-deck Tauri command and event prefixes:
 * - `main` → `main_deck_*` commands, `main-deck:*` events
 * - `cue`  → `cue_*` commands, `cue:*` events
 */
export type DeckId = "main" | "cue";

interface DeckIpc {
  commandPrefix: string;
  eventPrefix: string;
}

function ipcFor(deckId: DeckId): DeckIpc {
  return deckId === "main"
    ? { commandPrefix: "main_deck", eventPrefix: "main-deck" }
    : { commandPrefix: "cue", eventPrefix: "cue" };
}

export class NativeBackend implements DeckBackend {
  private handlers = new Set<DeckEventHandler>();
  private unlisteners: UnlistenFn[] = [];
  private subscribed: Promise<void>;
  private ipc: DeckIpc;

  constructor(deckId: DeckId = "main") {
    this.ipc = ipcFor(deckId);
    this.subscribed = this.subscribe();
  }

  private async subscribe(): Promise<void> {
    const p = this.ipc.eventPrefix;
    const subs = await Promise.all([
      listen<number>(`${p}:time`, (e) =>
        this.emit({ type: "time", seconds: e.payload }),
      ),
      listen<number>(`${p}:duration`, (e) =>
        this.emit({ type: "duration", seconds: e.payload }),
      ),
      listen<boolean>(`${p}:pause-state`, (e) =>
        this.emit({ type: "pause-state", paused: e.payload }),
      ),
      listen<null>(`${p}:ended`, () => this.emit({ type: "ended" })),
      listen<boolean>(`${p}:buffering`, (e) =>
        this.emit({ type: "buffering", buffering: e.payload }),
      ),
      listen<number[]>(`${p}:cache-state`, (e) =>
        this.emit({ type: "cache-state", ids: e.payload }),
      ),
      listen<string>(`${p}:error`, (e) =>
        this.emit({ type: "error", message: e.payload }),
      ),
      listen<number>(`${p}:load-failed`, (e) =>
        this.emit({ type: "load-failed", id: e.payload }),
      ),
      listen<null>(`${p}:prefetch-failed`, () =>
        this.emit({ type: "prefetch-failed" }),
      ),
      listen<boolean>(`${p}:output-unavailable`, (e) =>
        this.emit({ type: "output-unavailable", unavailable: e.payload }),
      ),
    ]);
    this.unlisteners.push(...subs);
  }

  whenReady(): Promise<void> {
    return this.subscribed;
  }

  async load(trackId: number, cuePoints?: CuePoints | null): Promise<void> {
    await this.subscribed;
    await invoke<void>(`${this.ipc.commandPrefix}_load`, {
      id: trackId,
      cuePoints: cuePoints ?? null,
    });
  }

  play(): Promise<void> {
    return invoke<void>(`${this.ipc.commandPrefix}_play`);
  }

  pause(): Promise<void> {
    return invoke<void>(`${this.ipc.commandPrefix}_pause`);
  }

  stop(): Promise<void> {
    return invoke<void>(`${this.ipc.commandPrefix}_stop`);
  }

  seek(seconds: number): Promise<void> {
    return invoke<void>(`${this.ipc.commandPrefix}_seek`, { seconds });
  }

  setVolume(volume: number): Promise<void> {
    return invoke<void>(`${this.ipc.commandPrefix}_set_volume`, { volume });
  }

  on(handler: DeckEventHandler): () => void {
    this.handlers.add(handler);
    return () => {
      this.handlers.delete(handler);
    };
  }

  async dispose(): Promise<void> {
    for (const u of this.unlisteners) u();
    this.unlisteners = [];
    this.handlers.clear();
  }

  private emit(event: DeckEvent): void {
    for (const h of this.handlers) h(event);
  }
}
