import { readdirSync, readFileSync } from "node:fs";
import { join, relative, resolve } from "node:path";

import { describe, expect, it } from "vitest";

const ROOT = resolve(import.meta.dirname, "..");
const SRC = join(ROOT, "src");
const TAURI = join(ROOT, "src-tauri");

/** Colour tokens only: spacing, radius and fonts are deliberately not themeable. */
const NOT_A_COLOUR = /^--(sp-|r$|r-|font-)/;

/**
 * Colour literals: hex (3–8 digits), `rgb()`/`rgba()`, `hsl()`/`hsla()`.
 *
 * The hex arm refuses a trailing word character so an id selector is not
 * mistaken for a colour — `#fade-controls` would otherwise read as `#fade`.
 */
const COLOUR_LITERAL =
  /#[0-9a-fA-F]{3,8}(?![0-9a-zA-Z_-])|\b(?:rgba?|hsla?)\(/g;

/** CSS comments carry issue references like `(#279)`, which are not colours. */
function findLiterals(css: string): string[] {
  const withoutComments = css.replace(/\/\*[\s\S]*?\*\//g, "");
  return [...withoutComments.matchAll(COLOUR_LITERAL)].map((m) => m[0]);
}

function svelteFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return svelteFiles(path);
    return entry.name.endsWith(".svelte") ? [path] : [];
  });
}

describe("theme contract", () => {
  const styles = readFileSync(join(SRC, "styles.css"), "utf8");

  it("defines every colour in one :root block", () => {
    const root = styles.match(/^:root \{[\s\S]*?^\}/m);
    expect(root, "styles.css must carry a :root token block").not.toBeNull();

    expect(findLiterals(styles.replace(root![0], ""))).toEqual([]);
  });

  it("puts no colour literal in a component style block", () => {
    const offenders = svelteFiles(SRC).flatMap((path) => {
      const source = readFileSync(path, "utf8");
      const literals = [
        ...source.matchAll(/<style[^>]*>([\s\S]*?)<\/style>/g),
      ].flatMap((block) => findLiterals(block[1]));
      return literals.length
        ? [`${relative(SRC, path)}: ${literals.join(", ")}`]
        : [];
    });

    expect(offenders).toEqual([]);
  });

  /**
   * A colour token must exist in three places at once — the stylesheet, the
   * Rust allowlist, and every built-in — or a theme can set something that
   * paints nothing, or a built-in can have a hole for others to fall into.
   */
  it("agrees on the token contract across all three places", () => {
    const root = styles.match(/^:root \{[\s\S]*?^\}/m)![0];
    const css = [...root.matchAll(/^\s*(--[a-z0-9-]+):/gm)]
      .map((m) => m[1])
      .filter((token) => !NOT_A_COLOUR.test(token));

    const rust = [
      ...readFileSync(join(TAURI, "src/appearance/theme.rs"), "utf8")
        .split("THEMEABLE_TOKENS")[1]
        .split("];")[0]
        .matchAll(/"(--[a-z0-9-]+)"/g),
    ].map((m) => m[1]);

    expect(new Set(rust)).toEqual(new Set(css));

    for (const name of ["midnight", "daylight"]) {
      const theme = JSON.parse(
        readFileSync(join(TAURI, "themes", `${name}.json`), "utf8"),
      ) as { tokens: Record<string, string> };
      expect(
        new Set(Object.keys(theme.tokens)),
        `${name}.json must set every token — it is what others fall back to`,
      ).toEqual(new Set(css));
    }
  });

  it("catches a literal that slips in", () => {
    expect(findLiterals(".x { color: #ff0000; }")).toEqual(["#ff0000"]);
    expect(findLiterals(".x { color: rgba(0, 0, 0, 0.5); }")).toEqual([
      "rgba(",
    ]);
    expect(findLiterals("#fade-controls { color: var(--primary); }")).toEqual(
      [],
    );
    expect(findLiterals("/* markers (#279) */")).toEqual([]);
  });
});
