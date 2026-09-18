import { browser } from "@wdio/globals";
import fs from "fs/promises";
import path from "path";
import { E2E_APP_DATA_DIR } from "./wdio.conf";

export interface SeededConfig {
  musicPaths?: string[];
  commercialPaths?: string[];
  jinglePaths?: string[];
}

export interface LaunchedApp {
  appDataDir: string;
}

export async function launchApp(
  seeded: SeededConfig = {},
): Promise<LaunchedApp> {
  // Drop any state from a previous test: radiodiodj.db (tracks, FTS index,
  // WAL/SHM and migration backups), session.json (playlist/history), then
  // write a fresh config.json so the Rust backend reads the seeded paths at
  // startup. Fixture files are byte-identical across tests, so a surviving DB
  // would reattach them to the previous test's tracks by fingerprint.
  const entries = await fs.readdir(E2E_APP_DATA_DIR).catch(() => []);
  for (const name of entries) {
    if (name.startsWith("radiodiodj.") || name === "session.json") {
      await fs.rm(path.join(E2E_APP_DATA_DIR, name), { force: true });
    }
  }

  const config = {
    musicPaths: seeded.musicPaths ?? [],
    commercialPaths: seeded.commercialPaths ?? [],
    jinglePaths: seeded.jinglePaths ?? [],
  };
  await fs.writeFile(
    path.join(E2E_APP_DATA_DIR, "config.json"),
    JSON.stringify(config, null, 2),
  );

  // Reload the WebDriver session — tauri-driver kills the current app and
  // spawns a fresh one, which re-reads the seeded config.json on startup.
  await browser.reloadSession();
  await browser.$("#track-list").waitForExist({ timeout: 15_000 });

  return { appDataDir: E2E_APP_DATA_DIR };
}

export async function captureArtifacts(specName: string): Promise<void> {
  const resultsDir = path.join(
    process.cwd(),
    "e2e-results",
    sanitize(specName),
  );
  await fs.mkdir(resultsDir, { recursive: true });
  try {
    await browser.saveScreenshot(path.join(resultsDir, "failure.png"));
  } catch {
    /* session may already be gone */
  }
  // Snapshot the app data dir for forensics. `logs/` holds RadiodioDJ.log,
  // which carries the renderer console too (`attachConsole()` in main.ts).
  await copyFiles(E2E_APP_DATA_DIR, resultsDir);
  await copyFiles(path.join(E2E_APP_DATA_DIR, "logs"), resultsDir);
}

async function copyFiles(from: string, to: string): Promise<void> {
  try {
    const entries = await fs.readdir(from);
    for (const entry of entries) {
      const src = path.join(from, entry);
      const stat = await fs.stat(src);
      if (stat.isFile()) {
        await fs.copyFile(src, path.join(to, entry));
      }
    }
  } catch {
    /* ignore */
  }
}

function sanitize(s: string): string {
  return s.replace(/[^a-z0-9._-]+/gi, "_").slice(0, 100);
}

/**
 * Wait until every CSS animation running inside `selector` has finished.
 *
 * WebdriverIO resolves an element's click point from its bounding box and then
 * dispatches the click as a separate command. A dialog that is still sliding in
 * moves between those two steps, so the click can land beside the button it
 * aimed at — silently, with no error. Waiting for the animation to settle
 * removes the race.
 */
export async function waitForAnimations(selector: string): Promise<void> {
  await browser.waitUntil(
    async () =>
      browser.execute((s) => {
        const el = document.querySelector(s);
        if (!el) return false;
        return el
          .getAnimations({ subtree: true })
          .every((a) => a.playState === "finished");
      }, selector),
    { timeout: 5_000, timeoutMsg: `animations in ${selector} did not settle` },
  );
}
