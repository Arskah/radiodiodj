import { browser, expect } from "@wdio/globals";
import { launchApp, captureArtifacts } from "../launch";
import { sel } from "../selectors";

describe("theming", () => {
  afterEach(async function () {
    if (this.currentTest?.state === "failed") {
      await captureArtifacts(this.currentTest.fullTitle());
    }
  });

  it("repaints the app when a theme is picked", async () => {
    await launchApp();

    await browser.$(sel.settingsButton).click();
    await browser.$(sel.settingsTab("Appearance")).click();

    const root = browser.$("html");
    await expect(root).toHaveAttribute("data-theme-base", "dark");

    // Selecting a theme applies it immediately — the running app is the preview.
    await browser.$(sel.themeRow("daylight")).click();
    await expect(root).toHaveAttribute("data-theme-base", "light");

    // The palette is inline custom properties on <html>, not a stylesheet swap.
    const background = await browser.execute(() =>
      document.documentElement.style.getPropertyValue("--background"),
    );
    expect(background).not.toBe("");

    await browser.$(sel.closeSettings).click();
  });

  it("lists the seeded example, so a first-run operator has something to copy", async () => {
    await launchApp();

    await browser.$(sel.settingsButton).click();
    await browser.$(sel.settingsTab("Appearance")).click();
    await browser.$(sel.reloadThemes).click();

    await expect(browser.$(sel.themeRow("example"))).toBeExisting();
  });
});
