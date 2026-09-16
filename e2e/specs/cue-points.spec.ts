import { browser, expect } from "@wdio/globals";
import { launchApp, captureArtifacts } from "../launch";
import { createFixtureLibrary, type FixtureLibrary } from "../fixtures";
import { sel } from "../selectors";

/**
 * Cue points (#279) end to end: place markers in the editor, save, and see the
 * library's duration column switch to air time. Save goes through
 * `set_cue_points`, so a round trip that survives a reopen proves the markers
 * reached the `tracks` row and came back clamped.
 */
describe("cue points", () => {
  let fixtures: FixtureLibrary | null = null;

  afterEach(async function () {
    if (this.currentTest?.state === "failed") {
      await captureArtifacts(this.currentTest.fullTitle());
    }
    if (fixtures) await fixtures.cleanup();
    fixtures = null;
  });

  async function bootAndScan(): Promise<void> {
    fixtures = await createFixtureLibrary({
      music: [{ name: "cue-fixture", durationSec: 60 }],
    });
    await launchApp({ musicPaths: [fixtures.musicDir] });

    await browser.$(sel.settingsButton).click();
    await browser.$(sel.scanButton).waitForClickable({ timeout: 5_000 });
    await browser.$(sel.scanButton).click();
    await browser.waitUntil(
      async () => (await browser.$$(sel.trackRow).length) >= 1,
      { timeout: 15_000, timeoutMsg: "scan did not populate track list" },
    );
  }

  // Read through the DOM: under WebKitGTK getText()/getValue() occasionally
  // return "" for an element Svelte has bound but not yet laid out.
  async function rowDuration(): Promise<string> {
    return browser.execute(() => {
      const el = document.querySelector(".track-row .track-duration");
      return (el?.textContent ?? "").trim();
    });
  }

  async function fieldValue(marker: string): Promise<string> {
    return browser.execute((s) => {
      const el = document.querySelector(s) as HTMLInputElement | null;
      return el?.value ?? "";
    }, sel.cuePointField(marker));
  }

  /** Set a millisecond field the way a keystroke would, so `bind` sees it. */
  async function setField(marker: string, ms: number): Promise<void> {
    await browser.execute(
      (s, value) => {
        const el = document.querySelector(s) as HTMLInputElement | null;
        if (!el) return;
        el.value = value;
        el.dispatchEvent(new Event("input", { bubbles: true }));
      },
      sel.cuePointField(marker),
      String(ms),
    );
  }

  async function openCuePoints(): Promise<void> {
    await browser.$(sel.trackRow).click({ button: "right" });
    await browser.$(sel.contextMenu).waitForExist({ timeout: 5_000 });
    await browser
      .$(sel.contextMenu)
      .$(sel.contextMenuItem("Cue points"))
      .click();
    await browser.$(sel.cuePointDialog).waitForExist({ timeout: 5_000 });
  }

  it("trims a track and reports air time in the library", async () => {
    await bootAndScan();
    expect(await rowDuration()).toBe("1:00");

    await openCuePoints();
    await setField("cue_in_ms", 10_000);
    await setField("cue_out_ms", 30_000);
    await browser.$(sel.cuePointSave).click();
    await browser.$(sel.cuePointDialog).waitForExist({
      reverse: true,
      timeout: 5_000,
    });

    await browser.waitUntil(async () => (await rowDuration()) === "0:20", {
      timeout: 5_000,
      timeoutMsg: "library row did not switch to air time",
    });

    // Reopening reads the markers back off the track row, so this covers the
    // DB round trip rather than the local patch `saveCuePoints` applies.
    await openCuePoints();
    expect(await fieldValue("cue_in_ms")).toBe("10000");
    expect(await fieldValue("cue_out_ms")).toBe("30000");
    await browser.$(sel.cuePointClose).click();
    await browser.$(sel.cuePointDialog).waitForExist({
      reverse: true,
      timeout: 5_000,
    });
  });

  it("clamps a marker that falls outside the file", async () => {
    await bootAndScan();
    await openCuePoints();

    // Past the end of a 60s file: the backend bounds it and returns what it
    // stored, which is what the reopened dialog shows.
    await setField("cue_out_ms", 999_000);
    await browser.$(sel.cuePointSave).click();
    await browser.$(sel.cuePointDialog).waitForExist({
      reverse: true,
      timeout: 5_000,
    });

    await openCuePoints();
    const stored = Number(await fieldValue("cue_out_ms"));
    expect(stored).toBeLessThanOrEqual(60_000);
    expect(stored).toBeGreaterThan(0);
  });
});
