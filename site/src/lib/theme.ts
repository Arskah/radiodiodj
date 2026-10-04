import daylight from "../../../src-tauri/themes/daylight.json";
import midnight from "../../../src-tauri/themes/midnight.json";

/**
 * Renders a theme's tokens as CSS declarations.
 * @param tokens Custom property names mapped to colour values.
 * @returns The declarations, without a selector.
 */
function declarations(tokens: Record<string, string>): string {
  return Object.entries(tokens)
    .map(([name, value]) => `${name}:${value};`)
    .join("");
}

/**
 * The app's built-in themes as a stylesheet: `midnight` by default, `daylight`
 * for a visitor whose system prefers light. See `docs/website.md#colours`.
 */
export const themeCss =
  `:root{color-scheme:dark;${declarations(midnight.tokens)}}` +
  `@media (prefers-color-scheme: light){:root{color-scheme:light;${declarations(daylight.tokens)}}}`;
