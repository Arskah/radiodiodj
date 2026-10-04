import { mount } from "svelte";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { attachConsole, error } from "@tauri-apps/plugin-log";
import App from "./App.svelte";
import { api } from "./shared/api";
import { app } from "./shared/state.svelte";
import { applyPaintHint } from "./shared/appearance";
// Self-hosted fonts + icons (bundled, no CDN — desktop app runs offline).
import "@fontsource/inter/400.css";
import "@fontsource/inter/500.css";
import "@fontsource/inter/600.css";
import "@fontsource/inter/700.css";
import "@fontsource/inter/800.css";
import "@fontsource/jetbrains-mono/500.css";
import "@fontsource/jetbrains-mono/700.css";
import "material-symbols/outlined.css";
import "./styles.css";

void attachConsole();

// A load the policy blocks fails without a trace on screen, and only in a
// built app: `pnpm dev` is served by Vite, which the policy never reaches.
document.addEventListener("securitypolicyviolation", (event) => {
  void error(
    `content security policy blocked ${event.effectiveDirective}: ${event.blockedURI}`,
  );
});

// Paint the last known background before anything else, so a light theme does
// not flash dark. A hint only — the real palette lands a moment later.
applyPaintHint();

// The one awaited load: the first painted frame should already be the
// operator's theme, and the round trip is over a blob resident in Rust memory.
await app.loadAppearance();

mount(App, { target: document.getElementById("app")! });

// Load user tuning first so runtime values (auto-playlist buffer, save
// throttle, retry backoffs) are in effect before session/playlist logic runs.
void app.loadTuning();
void app.search();
void app.loadStats();
void app.loadSession();
void app.hydrateScanStatus();
void app.hydrateWaveformStatus();
void app.loadHealth();
void app.loadSavedPlaylists();
void app.loadAudioConfig();
void app.loadAdmin();
void app.loadUpdate();

const win = getCurrentWindow();
let closing = false;
void win.onCloseRequested(async (event) => {
  if (closing) return;
  closing = true;
  event.preventDefault();
  try {
    await app.flushSave();
    await api.broadcastShutdown();
  } finally {
    await win.destroy();
  }
});
