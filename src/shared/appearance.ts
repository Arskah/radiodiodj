import type { Appearance, ThemeBase } from "./types";

/**
 * A pre-mount paint hint, so a light theme does not flash dark on launch.
 *
 * This is the only renderer-side persistence in the app, and it is a hint and
 * never a source of truth: nothing reads it once `get_appearance` lands, and if
 * it is missing, stale or garbage the worst case is the flash it exists to
 * prevent. Storage is unavailable in some contexts, so every access is guarded.
 */
const PAINT_HINT_KEY = "appearance-paint";

interface PaintHint {
  base: ThemeBase;
  background: string;
  surface: string;
}

/** Tokens worth painting before the real appearance arrives. */
const HINT_TOKENS = ["--background", "--surface"] as const;

export function readPaintHint(): PaintHint | null {
  try {
    const raw = localStorage.getItem(PAINT_HINT_KEY);
    if (!raw) return null;
    const hint = JSON.parse(raw) as Partial<PaintHint>;
    if (hint.base !== "light" && hint.base !== "dark") return null;
    if (!hint.background || !hint.surface) return null;
    return hint as PaintHint;
  } catch {
    return null;
  }
}

export function savePaintHint(appearance: Appearance): void {
  try {
    const [background, surface] = HINT_TOKENS.map(
      (token) => appearance.tokens[token] ?? "",
    );
    if (!background || !surface) return;
    localStorage.setItem(
      PAINT_HINT_KEY,
      JSON.stringify({ base: appearance.base, background, surface }),
    );
  } catch {
    // A viewer with site data blocked simply gets the flash.
  }
}

/** Paint the hint onto the document, before anything is mounted. */
export function applyPaintHint(): void {
  const hint = readPaintHint();
  if (!hint) return;
  const root = document.documentElement;
  root.style.setProperty("--background", hint.background);
  root.style.setProperty("--surface", hint.surface);
  root.dataset["themeBase"] = hint.base;
  root.style.colorScheme = hint.base;
}
