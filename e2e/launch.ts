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
  // Snapshot the app data dir for forensics.
  try {
    const entries = await fs.readdir(E2E_APP_DATA_DIR);
    for (const entry of entries) {
      const src = path.join(E2E_APP_DATA_DIR, entry);
      const stat = await fs.stat(src);
      if (stat.isFile()) {
        await fs.copyFile(src, path.join(resultsDir, entry));
      }
    }
  } catch {
    /* ignore */
  }
}

function sanitize(s: string): string {
  return s.replace(/[^a-z0-9._-]+/gi, "_").slice(0, 100);
}
