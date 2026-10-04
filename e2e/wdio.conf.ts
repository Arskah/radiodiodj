import { ChildProcess, spawn } from "child_process";
import fs from "fs";
import net from "net";
import os from "os";
import path from "path";

const REPO_ROOT = path.resolve(__dirname, "..");
const E2E_BINARY = process.env.E2E_BINARY ?? "debug";
export const APP_BINARY: string = path.join(
  REPO_ROOT,
  "src-tauri",
  "target",
  E2E_BINARY,
  "radiodiodj",
);
const RESULTS_DIR = path.join(REPO_ROOT, "e2e-results");
const APP_IDENTIFIER = "com.radiodiodj";

/**
 * One `XDG_DATA_HOME` per worker: this module is evaluated once in each worker
 * process, so spec files running side by side never share app data. Each test
 * rewrites `${XDG_DATA_HOME}/${APP_IDENTIFIER}/config.json` and forces an app
 * respawn via `browser.reloadSession()`. The env var is set BEFORE tauri-driver
 * spawns so the child app inherits it; tauri-driver does not honour mid-run
 * capability changes for `tauri:options.env`.
 */
export const E2E_XDG_DATA_HOME: string = fs.mkdtempSync(
  path.join(os.tmpdir(), "radiodiodj-e2e-xdg-"),
);
export const E2E_APP_DATA_DIR: string = path.join(
  E2E_XDG_DATA_HOME,
  APP_IDENTIFIER,
);
fs.mkdirSync(E2E_APP_DATA_DIR, { recursive: true });

const BASE_PORT = 4444;
const DRIVER_READY_TIMEOUT_MS = 10_000;

let tauriDriver: ChildProcess | null = null;

function stopDriver(): void {
  if (tauriDriver) {
    tauriDriver.kill("SIGTERM");
    tauriDriver = null;
  }
}

/**
 * Resolve once something accepts connections on `port`. tauri-driver takes a
 * moment to bind, and a session request sent before that fails the whole spec
 * file.
 */
async function waitForPort(port: number): Promise<void> {
  const deadline = Date.now() + DRIVER_READY_TIMEOUT_MS;
  for (;;) {
    const open = await new Promise<boolean>((resolve) => {
      const socket = net.connect(port, "127.0.0.1");
      socket.once("connect", () => {
        socket.destroy();
        resolve(true);
      });
      socket.once("error", () => {
        socket.destroy();
        resolve(false);
      });
    });
    if (open) return;
    if (Date.now() > deadline) {
      throw new Error(`tauri-driver did not listen on port ${port}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

export const config: WebdriverIO.Config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: BASE_PORT,
  path: "/",
  tsConfigPath: path.join(__dirname, "tsconfig.json"),
  specs: [path.join(__dirname, "specs", "**", "*.spec.ts")],
  maxInstances: Number(process.env.E2E_WORKERS ?? 4),
  capabilities: [
    {
      // TEMP diagnostic toggle
      ...(process.env.E2E_CLASSIC
        ? { "wdio:enforceWebDriverClassic": true }
        : {}),
      "tauri:options": {
        application: APP_BINARY,
      },
    } as WebdriverIO.Capabilities,
  ],
  logLevel: "info",
  outputDir: RESULTS_DIR,
  bail: 0,
  waitforTimeout: 10_000,
  connectionRetryTimeout: 120_000,
  connectionRetryCount: 0,
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: {
    ui: "bdd",
    timeout: 90_000,
    retries: 0,
  },

  onPrepare: () => {
    if (!fs.existsSync(APP_BINARY)) {
      throw new Error(
        `e2e binary not found at ${APP_BINARY}\n` +
          `Build first: cargo build --manifest-path src-tauri/Cargo.toml ` +
          `(or E2E_BINARY=release for release build)`,
      );
    }
    fs.rmSync(RESULTS_DIR, { recursive: true, force: true });
    fs.mkdirSync(RESULTS_DIR, { recursive: true });
  },

  beforeSession: async (config, _capabilities, _specs, cid) => {
    // Each worker runs its own tauri-driver, which in turn owns a native
    // WebKitWebDriver, so a worker needs two ports to itself.
    const worker = Number(cid.split("-")[1] ?? 0);
    const port = BASE_PORT + worker * 2;
    config.port = port;

    process.env.XDG_DATA_HOME = E2E_XDG_DATA_HOME;
    process.env.RUST_LOG = process.env.RUST_LOG ?? "debug";
    process.env.RUST_BACKTRACE = process.env.RUST_BACKTRACE ?? "1";

    tauriDriver = spawn(
      "tauri-driver",
      ["--port", String(port), "--native-port", String(port + 1)],
      { stdio: ["ignore", "pipe", "pipe"], env: process.env },
    );
    // A worker that fails to open its session never reaches afterSession.
    process.once("exit", stopDriver);
    const logPath = path.join(RESULTS_DIR, `tauri-driver-${cid}.log`);
    const logStream = fs.createWriteStream(logPath, { flags: "a" });
    tauriDriver.stdout?.pipe(logStream);
    tauriDriver.stderr?.pipe(logStream);

    await waitForPort(port);
  },

  afterSession: stopDriver,
};
