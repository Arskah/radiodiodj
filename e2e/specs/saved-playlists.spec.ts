import { browser, expect } from "@wdio/globals";
import { launchApp, captureArtifacts } from "../launch";
import { createFixtureLibrary, type FixtureLibrary } from "../fixtures";
import { sel } from "../selectors";

describe("saved playlists", () => {
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
      music: [
        { name: "alpha-song" },
        { name: "beta-song" },
        { name: "gamma-track" },
      ],
    });
    await launchApp({ musicPaths: [fixtures.musicDir] });

    await browser.$(sel.settingsButton).click();
    await browser.$(sel.scanButton).waitForClickable({ timeout: 5_000 });
    await browser.$(sel.scanButton).click();
    await browser.waitUntil(
      async () => (await browser.$$(sel.trackRow).length) >= 3,
      { timeout: 15_000, timeoutMsg: "scan did not populate track list" },
    );
    await browser.waitUntil(
      async () =>
        ((await browser.$("#settings-overlay").getAttribute("class")) ?? "")
          .split(/\s+/)
          .includes("hidden"),
      { timeout: 15_000, timeoutMsg: "scan did not finish" },
    );
  }

  const queuedTitles = (): Promise<string[]> =>
    browser.execute(() =>
      Array.from(document.querySelectorAll("#playlist .pl-title")).map((el) =>
        (el.textContent ?? "").trim(),
      ),
    );

  const entryTitles = (): Promise<string[]> =>
    browser.execute(() =>
      Array.from(document.querySelectorAll(".saved-entry-title")).map((el) =>
        (el.textContent ?? "").trim(),
      ),
    );

  // Read through the DOM: under WebKitGTK `getText()` can return "" for an
  // element Svelte has bound but not yet laid out.
  const textOf = (selector: string): Promise<string> =>
    browser.execute(
      (s) =>
        (document.querySelector(s)?.textContent ?? "").replace(/\s+/g, " "),
      selector,
    );

  async function waitForText(selector: string, text: string): Promise<void> {
    await browser.waitUntil(
      async () => (await textOf(selector)).includes(text),
      {
        timeout: 5_000,
        timeoutMsg: `${selector} never read "${text}"`,
      },
    );
  }

  async function waitForQueue(length: number): Promise<void> {
    await browser.waitUntil(
      async () => (await queuedTitles()).length === length,
      { timeout: 5_000, timeoutMsg: `the playlist never held ${length} rows` },
    );
  }

  it("keeps the playlist as a saved playlist and queues it again", async () => {
    await bootAndScan();

    const rows = browser.$$(sel.trackRow);
    await rows[1].click();
    await rows[0].click();
    await browser.$(sel.addSelection).click();
    await waitForQueue(2);
    const queued = await queuedTitles();

    await browser.$(sel.savePlaylistAs).click();
    await browser.$(sel.savedDialog).waitForDisplayed({ timeout: 5_000 });
    await browser.$(sel.savedName).setValue("Morning show");
    await browser.$(sel.savedConfirm).click();
    await browser
      .$(sel.savedDialog)
      .waitForExist({ timeout: 5_000, reverse: true });

    await browser.$(sel.clearPlaylist).click();
    await waitForQueue(0);

    await browser.$(sel.libraryTab("Playlists")).click();
    const saved = browser.$(sel.savedRow);
    await saved.waitForDisplayed({ timeout: 5_000 });
    await waitForText(sel.savedRow, "Morning show");
    await waitForText(sel.savedRow, "2 tracks");

    await saved.click();
    await browser.waitUntil(async () => (await entryTitles()).length === 2, {
      timeout: 5_000,
      timeoutMsg: "the saved playlist did not open with its two entries",
    });
    expect(await entryTitles()).toEqual(queued);

    await browser.$(sel.savedAdd).click();
    await waitForQueue(2);
    expect(await queuedTitles()).toEqual(queued);
    await waitForText(sel.savedNotice, "Added 2 tracks");
  });

  it("adds a library track to a saved playlist from its menu", async () => {
    await bootAndScan();

    const row = browser.$$(sel.trackRow)[2];
    const title = await browser.execute(
      () =>
        document
          .querySelectorAll(".track-row")[2]
          ?.querySelector(".track-title")
          ?.textContent?.trim() ?? "",
    );
    await row.click({ button: "right" });
    const menu = browser.$(sel.contextMenu);
    await menu.waitForDisplayed({ timeout: 5_000 });
    await menu.$(sel.contextMenuItem("Add to saved playlist")).click();

    await browser.$(sel.savedDialog).waitForDisplayed({ timeout: 5_000 });
    // Lower case on purpose: after a right-click the driver drops the Shift of
    // a leading capital, and "Picks" arrives as "picks".
    await browser.$(sel.savedName).setValue("picks");
    await browser.$(sel.savedConfirm).click();
    await browser
      .$(sel.savedDialog)
      .waitForExist({ timeout: 5_000, reverse: true });

    await browser.$(sel.libraryTab("Playlists")).click();
    const saved = browser.$(sel.savedRow);
    await saved.waitForDisplayed({ timeout: 5_000 });
    await waitForText(sel.savedRow, "picks");
    await waitForText(sel.savedRow, "1 track");
    await saved.click();
    await browser.waitUntil(async () => (await entryTitles()).length === 1, {
      timeout: 5_000,
      timeoutMsg: "the saved playlist did not open with its entry",
    });
    expect(await entryTitles()).toEqual([title]);
  });
});
