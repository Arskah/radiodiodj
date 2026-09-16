import { browser, expect } from "@wdio/globals";
import { launchApp, captureArtifacts } from "../launch";
import { createFixtureLibrary, type FixtureLibrary } from "../fixtures";
import { sel } from "../selectors";

/**
 * Cue points (#279), end to end.
 *
 * The unit suites stop at a boundary: `cue_points.rs` and `envelope.rs` are
 * tested as pure functions and `state.svelte.ts` against a mock deck, so
 * nothing covers editor → `set_cue_points` → SQLite → `Cmd::Load` →
 * `take_duration` → `:ended` → auto-advance.
 *
 * The journey below covers exactly that chain, and its payload is the last
 * assertion: a 30-second file that hands over after 5 because `cue_out_ms` says
 * so. Nothing here listens to audio — the proof is timing. A player that
 * ignored the markers would run the full 30 seconds and time the wait out.
 */
describe("cue points", () => {
  let fixtures: FixtureLibrary | null = null;

  /** All five markers, in valid order — bounding is the clamp test's job. */
  const MARKERS = {
    cue_in_ms: 1_000,
    fade_in_ms: 2_000,
    fade_out_ms: 5_000,
    cue_out_ms: 6_000,
    next_start_ms: 5_500,
  };
  /** cue_out − cue_in, against a 30 s file. */
  const AIR_SECONDS = 5;

  afterEach(async function () {
    if (this.currentTest?.state === "failed") {
      await captureArtifacts(this.currentTest.fullTitle());
    }
    if (fixtures) await fixtures.cleanup();
    fixtures = null;
  });

  // ----- DOM helpers -----
  //
  // Everything reads and writes through `browser.execute`, following
  // metadata.spec.ts: under WebKitGTK getText()/getValue() intermittently
  // return "" for an element Svelte has bound but not yet laid out, and
  // clearValue()/setValue() skip the `input` event `bind:value` listens for.

  /** `aria-label` of one element — how a button's state is read here. */
  async function label(selector: string): Promise<string> {
    return browser.execute(
      (s) => document.querySelector(s)?.getAttribute("aria-label") ?? "",
      selector,
    );
  }

  async function text(selector: string): Promise<string> {
    return browser.execute(
      (s) => (document.querySelector(s)?.textContent ?? "").trim(),
      selector,
    );
  }

  async function count(selector: string): Promise<number> {
    return browser.execute(
      (s) => document.querySelectorAll(s).length,
      selector,
    );
  }

  async function fieldValue(marker: string): Promise<string> {
    return browser.execute((s) => {
      const el = document.querySelector(s) as HTMLInputElement | null;
      return el?.value ?? "";
    }, sel.cuePointField(marker));
  }

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

  async function selectOption(selector: string, index: number): Promise<void> {
    await browser.execute(
      (s, i) => {
        const el = document.querySelector(s) as HTMLSelectElement | null;
        if (!el) return;
        el.value = el.options[i].value;
        el.dispatchEvent(new Event("change", { bubbles: true }));
      },
      selector,
      index,
    );
  }

  async function rowTitles(): Promise<string[]> {
    return browser.execute(() =>
      Array.from(document.querySelectorAll(".track-row .track-title")).map(
        (el) => (el.textContent ?? "").trim(),
      ),
    );
  }

  /** Default sort is whatever SQLite returns, so no spec may assume row 0. */
  async function rowIndexByTitle(title: string): Promise<number> {
    const i = (await rowTitles()).indexOf(title);
    expect(i).toBeGreaterThanOrEqual(0);
    return i;
  }

  /** Seconds from a `m:ss` string; -1 when the text is not a time yet. */
  function seconds(stamp: string): number {
    const [mm, ss] = stamp.trim().split(":").map(Number);
    if (!Number.isFinite(mm) || !Number.isFinite(ss)) return -1;
    return mm * 60 + ss;
  }

  /** A deck's time pill reads `current / total`. */
  async function pill(
    selector: string,
  ): Promise<{ current: number; total: number }> {
    const [current, total] = (await text(selector)).split("/");
    return { current: seconds(current ?? ""), total: seconds(total ?? "") };
  }

  /** The duration cell of one playlist row, in seconds. */
  async function playlistRowSeconds(index: number): Promise<number> {
    const stamp = await browser.execute((i) => {
      const row = document.querySelectorAll("#playlist .playlist-row")[i];
      return (row?.querySelector(".pl-duration")?.textContent ?? "").trim();
    }, index);
    return seconds(stamp);
  }

  /** The duration cell of one library row, in seconds. */
  async function rowDuration(index: number): Promise<number> {
    const stamp = await browser.execute((i) => {
      const row = document.querySelectorAll(".track-row")[i];
      return (row?.querySelector(".track-duration")?.textContent ?? "").trim();
    }, index);
    return seconds(stamp);
  }

  // ----- Flow helpers -----

  async function bootAndScan(): Promise<void> {
    fixtures = await createFixtureLibrary({
      music: [
        { name: "cue-fixture", durationSec: 30 },
        { name: "next-fixture", durationSec: 20 },
      ],
    });
    await launchApp({ musicPaths: [fixtures.musicDir] });

    await browser.$(sel.settingsButton).click();
    await browser.$(sel.scanButton).waitForClickable({ timeout: 5_000 });
    // Scanning closes the overlay itself.
    await browser.$(sel.scanButton).click();
    await browser.waitUntil(
      async () => (await browser.$$(sel.trackRow).length) >= 2,
      { timeout: 15_000, timeoutMsg: "scan did not populate track list" },
    );
  }

  /**
   * The cue deck is hidden until an output device is picked. Main and cue may
   * share one device — there is no exclusive mode (#95) — so the first real
   * entry does, which is also how the feature gets tested without headphones.
   */
  async function enableCueDeck(): Promise<void> {
    await browser.$(sel.settingsButton).click();
    await browser.$(sel.settingsTab("Audio Output")).click();
    await browser.$(sel.cueDeviceSelect).waitForExist({ timeout: 5_000 });

    // Option 0 is "Disabled", so anything real comes after it. Assert rather
    // than skip: CI runs PulseAudio with a null sink bridged to ALSA, and an
    // empty list is a regression, not an environment quirk.
    const options = await browser.execute((s) => {
      const el = document.querySelector(s) as HTMLSelectElement | null;
      return el ? el.options.length : 0;
    }, sel.cueDeviceSelect);
    expect(options).toBeGreaterThan(1);

    await selectOption(sel.cueDeviceSelect, 1);
    await browser.$(sel.closeSettings).click();
    // Applied live — no restart, unlike the main device.
    await browser.$(sel.cueDeck).waitForExist({ timeout: 5_000 });
  }

  async function openCuePoints(rowIndex: number): Promise<void> {
    const rows = await browser.$$(sel.trackRow);
    await rows[rowIndex].click({ button: "right" });
    await browser.$(sel.contextMenu).waitForExist({ timeout: 5_000 });
    await browser
      .$(sel.contextMenu)
      .$(sel.contextMenuItem("Cue points"))
      .click();
    await browser.$(sel.cuePointDialog).waitForExist({ timeout: 5_000 });
  }

  async function closeCuePoints(selector: string): Promise<void> {
    await browser.$(selector).click();
    await browser.$(sel.cuePointDialog).waitForExist({
      reverse: true,
      timeout: 5_000,
    });
  }

  it("edits, auditions, promotes and airs a trimmed track", async () => {
    // 1 — a 30 s file, untouched.
    await bootAndScan();
    await enableCueDeck();
    const edited = await rowIndexByTitle("cue-fixture");
    expect(await rowDuration(edited)).toBe(30);

    // 1b — cue it first, the way an operator reaches the editor. The dialog
    // borrows the cue deck and hands it back on exit, so what is cued here is
    // what is still cued in step 6.
    const libraryRow = await browser.$$(sel.trackRow);
    await libraryRow[edited].$(sel.trackRowCue).click();
    await browser.$(sel.cuePromote).waitForEnabled({ timeout: 5_000 });

    // 2 — set all five markers.
    await openCuePoints(edited);
    for (const [marker, ms] of Object.entries(MARKERS)) {
      await setField(marker, ms);
    }
    // One line per set marker, and no more: an unset marker resolves onto a
    // neighbour and is deliberately not drawn.
    expect(await count(sel.cuePointMarker)).toBe(5);

    // 3 — audition the draft before it is saved, without leaving the dialog.
    // This is what `cue_load`'s optional `cuePoints` and its `autoplay` exist
    // for: the deck reports the draft's air time and actually plays it.
    await browser.$(sel.cuePointAudition).click();
    await browser.waitUntil(
      async () => (await pill(sel.cueTimeDisplay)).total === AIR_SECONDS,
      { timeout: 10_000, timeoutMsg: "cue deck did not report air time" },
    );
    expect(await text(sel.cueModeActive)).toBe("Preview");
    await browser.waitUntil(
      async () => (await label(sel.cuePlay)) === "Pause cue",
      {
        timeout: 10_000,
        timeoutMsg: "auditioning did not start playback — it only staged it",
      },
    );
    // Still open: the whole point of the audition living in the dialog.
    expect(await browser.$(sel.cuePointDialog).isExisting()).toBe(true);

    // 4 — save, and the library column switches to air time.
    await closeCuePoints(sel.cuePointSave);
    await browser.waitUntil(
      async () => (await rowDuration(edited)) === AIR_SECONDS,
      { timeout: 5_000, timeoutMsg: "library row did not switch to air time" },
    );

    // 5 — reopening reads the markers back off the track row, so this covers
    // the DB round trip rather than the local patch `saveCuePoints` applies.
    await openCuePoints(edited);
    for (const [marker, ms] of Object.entries(MARKERS)) {
      expect(await fieldValue(marker)).toBe(String(ms));
    }
    await closeCuePoints(sel.cuePointClose);

    // 6 — promote the cued track to the head of the playlist, and queue a
    // second one behind it so the handover has somewhere to go.
    await browser.$(sel.cuePromote).click();
    await browser.$(`${sel.playlist} ${sel.playlistRow}`).waitForExist({
      timeout: 5_000,
    });
    expect(await text(`${sel.playlist} ${sel.playlistRow} .pl-title`)).toBe(
      "cue-fixture",
    );
    expect(
      seconds(
        await text(
          `${sel.playlist} ${sel.playlistRow} ${sel.playlistRowDuration}`,
        ),
      ),
    ).toBe(AIR_SECONDS);

    const next = await rowIndexByTitle("next-fixture");
    const libraryRows = await browser.$$(sel.trackRow);
    await libraryRows[next].$(sel.trackRowAdd).click();
    await browser.waitUntil(
      async () =>
        (await browser.$$(`${sel.playlist} ${sel.playlistRow}`).length) >= 2,
      { timeout: 5_000, timeoutMsg: "second track never queued" },
    );

    // Silence the cue deck so only the main deck is playing from here.
    await browser.$(sel.cueStop).click();

    // 7 — air it. The deck reports air time, not the file length.
    const queued = await browser.$$(`${sel.playlist} ${sel.playlistRow}`);
    await queued[0].doubleClick();
    await browser.waitUntil(
      async () => (await text(sel.npTitle)) === "cue-fixture",
      { timeout: 10_000, timeoutMsg: "edited track never reached the deck" },
    );
    await browser.waitUntil(
      async () => (await pill(sel.timeDisplay)).total === AIR_SECONDS,
      { timeout: 10_000, timeoutMsg: "main deck did not report air time" },
    );
    await browser.waitUntil(
      async () => (await pill(sel.timeDisplay)).current > 0,
      { timeout: 10_000, timeoutMsg: "player time never advanced" },
    );

    // 8 — the payload. `take_duration(cueOut − pos)` runs the sink dry at the
    // out point, `:ended` fires, and the playlist advances. A player that
    // ignored the markers would still be on this track 25 seconds from now.
    await browser.waitUntil(
      async () => (await text(sel.npTitle)) === "next-fixture",
      {
        timeout: 15_000,
        timeoutMsg:
          "trimmed track did not end at its cue-out and hand over — " +
          "cue points are not reaching the player",
      },
    );
  });

  /**
   * Item overrides — the last slice of #279. Cue points that belong to one
   * queued airing rather than to the track, sent straight from the editor by
   * _Use once_. The library row is the control: it must still read 30 s
   * afterwards, because the track itself is never written to.
   */
  it("queues an unsaved edit for one airing", async () => {
    await bootAndScan();
    const edited = await rowIndexByTitle("cue-fixture");

    // Two airings of the same one-off: one gets handed back to the track, the
    // other goes on air still carrying it.
    for (let i = 0; i < 2; i += 1) {
      await openCuePoints(edited);
      await setField("cue_out_ms", 4_000);
      await closeCuePoints(sel.cuePointUseOnce);
    }
    await browser.waitUntil(
      async () =>
        (await browser.$$(`${sel.playlist} ${sel.playlistRow}`).length) >= 2,
      { timeout: 5_000, timeoutMsg: "Use once never reached the playlist" },
    );
    expect(await count(`${sel.playlist} ${sel.playlistRowOverride}`)).toBe(2);
    expect(await playlistRowSeconds(0)).toBe(4);
    // Nothing was saved, so the track still airs in full.
    expect(await rowDuration(edited)).toBe(30);

    // The badge hands one item back to the track's own cue points, which for an
    // unedited track means the whole file.
    await browser.$(`${sel.playlist} ${sel.playlistRowOverride}`).click();
    await browser.waitUntil(async () => (await playlistRowSeconds(0)) === 30, {
      timeout: 5_000,
      timeoutMsg: "clearing the override did not restore the track's duration",
    });
    expect(await count(`${sel.playlist} ${sel.playlistRowOverride}`)).toBe(1);

    // And the item that kept it reaches the player: the main deck reports the
    // override's air time, not the file's 30 seconds.
    const queued = await browser.$$(`${sel.playlist} ${sel.playlistRow}`);
    await queued[1].doubleClick();
    await browser.waitUntil(
      async () => (await pill(sel.timeDisplay)).total === 4,
      { timeout: 10_000, timeoutMsg: "override never reached the main deck" },
    );
  });

  it("clamps a marker that falls outside the file", async () => {
    await bootAndScan();
    const edited = await rowIndexByTitle("cue-fixture");
    await openCuePoints(edited);

    // Past the end of a 30s file: the backend bounds it and returns what it
    // stored, which is what the reopened dialog shows.
    await setField("cue_out_ms", 999_000);
    await closeCuePoints(sel.cuePointSave);

    await openCuePoints(edited);
    const stored = Number(await fieldValue("cue_out_ms"));
    expect(stored).toBeLessThanOrEqual(30_000);
    expect(stored).toBeGreaterThan(0);
    await closeCuePoints(sel.cuePointClose);
  });
});
