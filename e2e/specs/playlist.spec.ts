import { browser, expect } from "@wdio/globals";
import { launchApp, captureArtifacts } from "../launch";
import { createFixtureLibrary, type FixtureLibrary } from "../fixtures";
import { sel } from "../selectors";

describe("playlist", () => {
  let fixtures: FixtureLibrary | null = null;

  afterEach(async function () {
    if (this.currentTest?.state === "failed") {
      await captureArtifacts(this.currentTest.fullTitle());
    }
    if (fixtures) await fixtures.cleanup();
    fixtures = null;
  });

  it("auto-playlist toggle fills queue with lookahead tracks", async () => {
    const music = Array.from({ length: 10 }, (_, i) => ({
      name: `auto-${i}`,
    }));
    fixtures = await createFixtureLibrary({ music });
    await launchApp({ musicPaths: [fixtures.musicDir] });

    await browser.$(sel.settingsButton).click();
    await browser.$(sel.scanButton).waitForClickable({ timeout: 5_000 });
    await browser.$(sel.scanButton).click();
    await browser.waitUntil(
      async () => (await browser.$$(sel.trackRow).length) >= 10,
      { timeout: 15_000 },
    );

    const toggle = browser.$(sel.autoPlaylistToggle);
    await expect(toggle).toHaveAttribute("aria-pressed", "false");

    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-pressed", "true");

    await browser.waitUntil(
      async () =>
        (await browser.$$(`${sel.playlist} ${sel.playlistRow}`).length) >= 5,
      {
        timeout: 8_000,
        timeoutMsg: "auto-playlist did not populate lookahead",
      },
    );
  });

  /** A point on an element, `at` being how far down it as a fraction. */
  interface Hop {
    selector: string;
    index?: number;
    at?: number;
  }

  /**
   * Drag by dispatching the events a drag would. WebDriver's pointer actions
   * do not reliably start an HTML5 drag under WebKitGTK, and the handlers read
   * nothing from the event but the pointer's height.
   */
  async function drag(source: Hop, over: Hop[]): Promise<void> {
    await browser.execute(
      (source, over) => {
        const find = (hop: typeof source): Element => {
          const el = document.querySelectorAll(hop.selector)[hop.index ?? 0];
          if (!el) throw new Error(`no element for ${hop.selector}`);
          return el;
        };
        const fire = (type: string, hop: typeof source): void => {
          const el = find(hop);
          const rect = el.getBoundingClientRect();
          el.dispatchEvent(
            new MouseEvent(type, {
              bubbles: true,
              cancelable: true,
              clientY: rect.top + rect.height * (hop.at ?? 0.5),
            }),
          );
        };
        fire("dragstart", source);
        for (const hop of over) fire("dragover", hop);
        fire("drop", over[over.length - 1]);
        fire("dragend", source);
      },
      source,
      over,
    );
  }

  async function queuedTitles(): Promise<string[]> {
    return browser.execute(() =>
      Array.from(document.querySelectorAll("#playlist .pl-title")).map((el) =>
        (el.textContent ?? "").trim(),
      ),
    );
  }

  async function waitForQueue(expected: string[], what: string): Promise<void> {
    let seen: string[] = [];
    await browser
      .waitUntil(
        async () => {
          seen = await queuedTitles();
          return JSON.stringify(seen) === JSON.stringify(expected);
        },
        { timeout: 5_000 },
      )
      .catch(() => {
        throw new Error(`${what}: playlist is ${JSON.stringify(seen)}`);
      });
  }

  it("a library row dragged onto the playlist lands where it is dropped", async () => {
    fixtures = await createFixtureLibrary({
      music: [{ name: "drag-a" }, { name: "drag-b" }, { name: "drag-c" }],
    });
    await launchApp({ musicPaths: [fixtures.musicDir] });

    await browser.$(sel.settingsButton).click();
    await browser.$(sel.scanButton).waitForClickable({ timeout: 5_000 });
    await browser.$(sel.scanButton).click();
    await browser.waitUntil(
      async () => (await browser.$$(sel.trackRow).length) >= 3,
      { timeout: 15_000, timeoutMsg: "scan did not populate track list" },
    );
    // The overlay closes once the scan resolves; rows can be replaced by a
    // refresh until then.
    await browser.waitUntil(
      async () =>
        ((await browser.$("#settings-overlay").getAttribute("class")) ?? "")
          .split(/\s+/)
          .includes("hidden"),
      { timeout: 15_000, timeoutMsg: "scan did not finish" },
    );

    const [a, b, c] = await browser.execute(() =>
      Array.from(document.querySelectorAll(".track-row .track-title")).map(
        (el) => (el.textContent ?? "").trim(),
      ),
    );
    const libraryRow = (index: number): Hop => ({
      selector: sel.trackRow,
      index,
    });
    const queuedRow = (index: number, at: number): Hop => ({
      selector: `${sel.playlist} ${sel.playlistRow}`,
      index,
      at,
    });
    const list: Hop = { selector: sel.playlist };

    await drag(libraryRow(0), [list]);
    await waitForQueue([a], "drop into an empty playlist");

    await drag(libraryRow(1), [queuedRow(0, 0.25)]);
    await waitForQueue([b, a], "drop on the upper half of a row");

    await drag(libraryRow(2), [queuedRow(0, 0.75)]);
    await waitForQueue([b, c, a], "drop on the lower half of a row");

    // Passing over a row on the way must not leave the drop pointing at it.
    await drag(libraryRow(0), [queuedRow(0, 0.25), list]);
    await waitForQueue([b, c, a, a], "drop under the last row");

    await drag(queuedRow(0, 0.5), [queuedRow(1, 0.75)]);
    await waitForQueue([c, b, a, a], "reorder by dragging a queued row");
  });
});
