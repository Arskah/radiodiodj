import { browser, expect } from "@wdio/globals";
import { launchApp, captureArtifacts } from "../launch";
import { createFixtureLibrary, type FixtureLibrary } from "../fixtures";
import { sel } from "../selectors";

describe("metadata editing", () => {
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
      music: [{ name: "edit-fixture" }],
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

  // Read text/values via the DOM directly — under WebKitGTK getText()/getValue()
  // occasionally return "" for an element Svelte has bound but not yet laid out.
  async function rowTitles(): Promise<string[]> {
    return browser.execute(() =>
      Array.from(document.querySelectorAll(".track-row .track-title")).map(
        (el) => (el.textContent ?? "").trim(),
      ),
    );
  }

  async function inputValue(selector: string): Promise<string> {
    return browser.execute((s) => {
      const el = document.querySelector(s) as HTMLInputElement | null;
      return el?.value ?? "";
    }, selector);
  }

  // WebdriverIO's clearValue() sets the field empty without an `input` event, so
  // Svelte's bind:value never sees the change and keeps the old value. Clear via
  // the DOM and dispatch `input` so the binding updates like a real keystroke.
  async function clearInput(selector: string): Promise<void> {
    await browser.execute((s) => {
      const el = document.querySelector(s) as HTMLInputElement | null;
      if (!el) return;
      el.value = "";
      el.dispatchEvent(new Event("input", { bubbles: true }));
    }, selector);
  }

  async function openEditor(): Promise<void> {
    await browser.$(sel.editButton).click();
    await browser.$(sel.metadataDialog).waitForExist({ timeout: 5_000 });
    // The overlay animates in; wait until the title field is interactable.
    await browser.$(sel.metadataTitle).waitForClickable({ timeout: 5_000 });
  }

  it("edits title/genre/year and persists after reopening", async () => {
    await bootAndScan();

    await openEditor();
    // Untagged WAV → title defaults to the file basename, genre/year empty.
    expect(await inputValue(sel.metadataTitle)).toBe("edit-fixture");

    await browser.$(sel.metadataTitle).setValue("Edited Title");
    await browser.$(sel.metadataGenre).setValue("Techno");
    await browser.$(sel.metadataYear).setValue("1998");
    await browser.$(sel.metadataSave).click();

    // Save closes the dialog and the row reflects the new title.
    await browser.$(sel.metadataDialog).waitForExist({
      reverse: true,
      timeout: 5_000,
    });
    await browser.waitUntil(
      async () => {
        const titles = await rowTitles();
        return titles.length === 1 && titles[0] === "Edited Title";
      },
      { timeout: 5_000, timeoutMsg: "row title did not update after save" },
    );

    // Reopen — the values come back from the backend (get_track after UPDATE),
    // so seeing them proves the edit round-tripped through the DB, not just the
    // local row patch. Genre/year aren't shown in the row, so this is the only
    // check that covers them.
    await openEditor();
    expect(await inputValue(sel.metadataTitle)).toBe("Edited Title");
    expect(await inputValue(sel.metadataGenre)).toBe("Techno");
    expect(await inputValue(sel.metadataYear)).toBe("1998");
    await browser.$(sel.metadataCancel).click();
    await browser.$(sel.metadataDialog).waitForExist({
      reverse: true,
      timeout: 5_000,
    });
  });

  it("clears the genre when emptied and saved", async () => {
    await bootAndScan();

    // Seed a genre first.
    await openEditor();
    await browser.$(sel.metadataGenre).setValue("Ambient");
    await browser.$(sel.metadataSave).click();
    await browser.$(sel.metadataDialog).waitForExist({
      reverse: true,
      timeout: 5_000,
    });

    // Reopen, clear it, save.
    await openEditor();
    expect(await inputValue(sel.metadataGenre)).toBe("Ambient");
    await clearInput(sel.metadataGenre);
    await browser.$(sel.metadataSave).click();
    await browser.$(sel.metadataDialog).waitForExist({
      reverse: true,
      timeout: 5_000,
    });

    // Reopen — genre is now empty (cleared to NULL and reloaded as "").
    await openEditor();
    expect(await inputValue(sel.metadataGenre)).toBe("");
    await browser.$(sel.metadataCancel).click();
  });

  it("rejects an empty title and keeps the dialog open", async () => {
    await bootAndScan();

    await openEditor();
    await clearInput(sel.metadataTitle);
    await browser.$(sel.metadataSave).click();

    // Validation blocks the save: the dialog stays open and an error shows.
    await browser.$(sel.metadataError).waitForExist({ timeout: 5_000 });
    await expect(browser.$(sel.metadataError)).toHaveText(
      expect.stringContaining("Title"),
    );
    await expect(browser.$(sel.metadataDialog)).toExist();

    // The row title is untouched.
    const titles = await rowTitles();
    expect(titles[0]).toBe("edit-fixture");
  });
});
