import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";

export default defineConfig({
  plugins: [svelte({ configFile: "./svelte.config.mjs" })],
  // Mounted component tests need Svelte's client entry point in jsdom.
  resolve: { conditions: ["browser"] },
  test: {
    name: "renderer",
    environment: "jsdom",
    include: ["src/**/*.test.ts", "tools/**/*.test.ts"],
    coverage: {
      include: ["src/**/*.{ts,svelte}"],
    },
  },
});
