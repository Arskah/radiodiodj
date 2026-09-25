import { ChildProcess, spawn } from "child_process";
import fs from "fs";
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
 * Fixed `XDG_DATA_HOME` for the whole run. Each test rewrites
 * `${XDG_DATA_HOME}/${APP_IDENTIFIER}/config.json` and forces an app respawn
 * via `browser.reloadSession()`. The env var is set BEFORE tauri-driver
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

let tauriDriver: ChildProcess | null = null;

export const config: WebdriverIO.Config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  tsConfigPath: path.join(__dirname, "tsconfig.json"),
  specs: [path.join(__dirname, "specs", "**", "*.spec.ts")],
  maxInstances: 1,
  capabilities: [
    {
      maxInstances: 1,
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

  beforeSession: () => {
    process.env.XDG_DATA_HOME = E2E_XDG_DATA_HOME;
    process.env.RUST_LOG = process.env.RUST_LOG ?? "debug";
    process.env.RUST_BACKTRACE = process.env.RUST_BACKTRACE ?? "1";

    tauriDriver = spawn("tauri-driver", [], {
      stdio: ["ignore", "pipe", "pipe"],
      env: process.env,
    });
    const logPath = path.join(RESULTS_DIR, "tauri-driver.log");
    const logStream = fs.createWriteStream(logPath, { flags: "a" });
    tauriDriver.stdout?.pipe(logStream);
    tauriDriver.stderr?.pipe(logStream);
  },

  afterSession: () => {
    if (tauriDriver) {
      tauriDriver.kill("SIGTERM");
      tauriDriver = null;
    }
  },
};
