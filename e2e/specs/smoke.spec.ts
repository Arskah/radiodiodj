import { browser, expect } from "@wdio/globals";
import fs from "fs/promises";
import path from "path";
import { launchApp, captureArtifacts } from "../launch";
import { sel } from "../selectors";

describe("smoke", () => {
  afterEach(async function () {
    if (this.currentTest?.state === "failed") {
      await captureArtifacts(this.currentTest.fullTitle());
    }
  });

  it("boots with empty library and settings closed", async () => {
    await launchApp();

    await expect(browser.$(sel.trackList)).toBeExisting();
    await expect(browser.$("#settings-overlay")).toHaveAttribute(
      "class",
      expect.stringContaining("hidden"),
    );
    await expect(browser.$(`${sel.trackList} .empty .empty-title`)).toHaveText(
      "Your Library is Empty",
    );
  });

  it("boots without a content security policy violation", async () => {
    const { appDataDir } = await launchApp();
    await browser.$(sel.trackList).waitForExist();

    const log = await fs.readFile(
      path.join(appDataDir, "logs", "RadiodioDJ.log"),
      "utf8",
    );
    expect(log).not.toContain("content security policy blocked");
  });
});
