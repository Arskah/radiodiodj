import { browser, expect } from "@wdio/globals";
import { launchApp, captureArtifacts } from "../launch";
import { createFixtureLibrary, type FixtureLibrary } from "../fixtures";
import { sel } from "../selectors";

describe("library", () => {
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

    // Rows appear from scan progress, so the list can be complete while the
    // scan still runs and the settings overlay is still up. The overlay closes
    // only once `scan()` resolves, which makes it the signal that no further
    // refresh of the track list is pending — without it a test can act on rows
    // that a later refresh replaces underneath it.
    await browser.waitUntil(
      async () =>
        ((await browser.$("#settings-overlay").getAttribute("class")) ?? "")
          .split(/\s+/)
          .includes("hidden"),
      { timeout: 15_000, timeoutMsg: "scan did not finish" },
    );
  }

  it("scans a fixture library and lists tracks", async () => {
    await bootAndScan();

    const rows = browser.$$(sel.trackRow);
    await expect(rows).toBeElementsArrayOfSize(3);

    const statusBar = browser.$(sel.scanStatusBar);
    if (await statusBar.isExisting()) {
      await expect(statusBar).toHaveText(expect.stringContaining("Scan"));
    }
  });

  // Read rendered text via textContent rather than WebDriver's getText() —
  // under WebKitGTK, getText() occasionally returns "" for a span whose
  // text-node child has been bound by Svelte but not yet visually laid out,
  // even though the surrounding DOM is settled enough for findElement to
  // resolve. textContent reflects the current DOM state without the layout
  // dependency.
  async function rowTitles(): Promise<string[]> {
    return browser.execute(() =>
      Array.from(document.querySelectorAll(".track-row .track-title")).map(
        (el) => (el.textContent ?? "").trim(),
      ),
    );
  }

  it("filters via FTS prefix search", async () => {
    await bootAndScan();

    await browser.$(sel.searchInput).setValue("alpha");
    await browser.waitUntil(
      async () => {
        const titles = await rowTitles();
        return titles.length === 1 && titles[0] === "alpha-song";
      },
      {
        timeout: 5_000,
        timeoutMsg: "search did not filter to alpha-song",
      },
    );
  });

  it("re-orders rows when sort header clicked", async () => {
    await bootAndScan();

    const titleHeader = browser.$$('button[role="columnheader"]')[0];
    await titleHeader.click();
    await browser.waitUntil(
      async () => (await titleHeader.getAttribute("aria-sort")) === "ascending",
      { timeout: 5_000 },
    );
    let firstAsc = "";
    await browser.waitUntil(
      async () => {
        const titles = await rowTitles();
        if (titles.length === 0 || titles[0] === "") return false;
        firstAsc = titles[0];
        return true;
      },
      { timeout: 5_000, timeoutMsg: "ascending titles never rendered" },
    );

    await titleHeader.click();
    await browser.waitUntil(
      async () =>
        (await titleHeader.getAttribute("aria-sort")) === "descending",
      { timeout: 5_000 },
    );
    let firstDesc = "";
    await browser.waitUntil(
      async () => {
        const titles = await rowTitles();
        if (titles.length === 0 || titles[0] === "" || titles[0] === firstAsc)
          return false;
        firstDesc = titles[0];
        return true;
      },
      {
        timeout: 5_000,
        timeoutMsg: "descending titles did not flip from ascending order",
      },
    );

    await expect(firstAsc).not.toBe(firstDesc);
  });

  it("adds a track through the row context menu", async () => {
    await bootAndScan();

    await browser.$(sel.trackRow).click({ button: "right" });
    const menu = browser.$(sel.contextMenu);
    await menu.waitForDisplayed({ timeout: 5_000 });

    await menu.$(sel.contextMenuItem("Add to playlist")).click();
    await menu.waitForExist({ timeout: 5_000, reverse: true });

    await browser.waitUntil(
      async () =>
        (await browser.$$(`${sel.playlist} ${sel.playlistRow}`).length) === 1,
      {
        timeout: 5_000,
        timeoutMsg: "context menu add did not reach the playlist",
      },
    );
  });

  it("queues a track next-up through the row context menu", async () => {
    await bootAndScan();

    const rows = browser.$$(sel.trackRow);
    await rows[0].$(sel.trackRowAdd).click();
    const nextTitle = (await rowTitles())[1];

    await rows[1].click({ button: "right" });
    const menu = browser.$(sel.contextMenu);
    await menu.waitForDisplayed({ timeout: 5_000 });
    await menu.$(sel.contextMenuItem("Add as next")).click();
    await menu.waitForExist({ timeout: 5_000, reverse: true });

    await browser.waitUntil(
      async () => {
        const queued = await browser.execute(() =>
          Array.from(document.querySelectorAll("#playlist .pl-title")).map(
            (el) => (el.textContent ?? "").trim(),
          ),
        );
        return queued.length === 2 && queued[0] === nextTitle;
      },
      {
        timeout: 5_000,
        timeoutMsg: "add-as-next did not land at the head of the playlist",
      },
    );
  });

  it("opens the context menu from the keyboard", async () => {
    await bootAndScan();

    // Rows are tabbable, but reaching one costs a tab per row above it; focus
    // the first directly and exercise the binding itself. Focus is re-applied
    // until it sticks: the key event is delivered to whatever holds focus at
    // that moment, so a row re-render between the focus call and the keystroke
    // would otherwise send Shift+F10 to the body and time out on the menu.
    await browser.waitUntil(
      async () =>
        browser.execute(() => {
          const row = document.querySelector<HTMLElement>(".track-row");
          if (!row) return false;
          if (document.activeElement !== row) row.focus();
          return document.activeElement === row;
        }),
      { timeout: 5_000, timeoutMsg: "the first track row never took focus" },
    );
    await browser.keys(["Shift", "F10"]);

    const menu = browser.$(sel.contextMenu);
    await menu.waitForDisplayed({ timeout: 5_000 });
    // The first item takes focus, so Enter activates it without a pointer.
    await browser.keys("Enter");
    await menu.waitForExist({ timeout: 5_000, reverse: true });

    await browser.waitUntil(
      async () =>
        (await browser.$$(`${sel.playlist} ${sel.playlistRow}`).length) === 1,
      {
        timeout: 5_000,
        timeoutMsg: "keyboard context menu add did not reach the playlist",
      },
    );
  });

  it("dismisses the context menu on Escape", async () => {
    await bootAndScan();

    await browser.$(sel.trackRow).click({ button: "right" });
    const menu = browser.$(sel.contextMenu);
    await menu.waitForDisplayed({ timeout: 5_000 });

    await browser.keys("Escape");
    await menu.waitForExist({ timeout: 5_000, reverse: true });
  });
});
