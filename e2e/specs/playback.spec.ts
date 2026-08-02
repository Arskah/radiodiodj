import { browser, expect } from "@wdio/globals";
import { launchApp, captureArtifacts } from "../launch";
import { createFixtureLibrary, type FixtureLibrary } from "../fixtures";
import { sel } from "../selectors";

function parseTime(s: string): number {
  const head = s.split("/")[0].trim();
  const [mm, ss] = head.split(":").map(Number);
  return mm * 60 + ss;
}

describe("playback", () => {
  let fixtures: FixtureLibrary | null = null;

  afterEach(async function () {
    if (this.currentTest?.state === "failed") {
      await captureArtifacts(this.currentTest.fullTitle());
    }
    if (fixtures) await fixtures.cleanup();
    fixtures = null;
  });

  it("advances time on play and halts on pause", async () => {
    fixtures = await createFixtureLibrary({
      music: [{ name: "play-fixture", durationSec: 5.0 }],
    });
    await launchApp({ musicPaths: [fixtures.musicDir] });

    await browser.$(sel.settingsButton).click();
    await browser.$(sel.scanButton).waitForClickable({ timeout: 5_000 });
    await browser.$(sel.scanButton).click();
    await browser.waitUntil(
      async () => (await browser.$$(sel.trackRow).length) >= 1,
      { timeout: 15_000 },
    );

    // Add to playlist via the + button on the track row
    await browser.$(".btn-add").click();
    await browser.$(`${sel.playlist} ${sel.playlistRow}`).waitForExist({
      timeout: 5_000,
    });

    // Double-click playlist row to start playing
    await browser.$(`${sel.playlist} ${sel.playlistRow}`).doubleClick();

    // Wait until time display advances past 0
    await browser.waitUntil(
      async () => parseTime(await browser.$(sel.timeDisplay).getText()) > 0,
      { timeout: 8_000, timeoutMsg: "player time never advanced" },
    );

    const beforePause = parseTime(await browser.$(sel.timeDisplay).getText());

    // Pause
    await browser.$(sel.btnPlay).click();
    await browser.pause(800);
    const afterPauseA = parseTime(await browser.$(sel.timeDisplay).getText());
    await browser.pause(800);
    const afterPauseB = parseTime(await browser.$(sel.timeDisplay).getText());

    await expect(afterPauseA).toBe(afterPauseB);
    await expect(beforePause).toBeGreaterThan(0);
  });
});
