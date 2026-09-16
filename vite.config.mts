import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { resolve } from "node:path";

const host = process.env["TAURI_DEV_HOST"];

export default defineConfig({
  publicDir: false,
  plugins: [
    svelte({ configFile: resolve(import.meta.dirname, "svelte.config.mjs") }),
  ],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    host: host ?? false,
    hmr: host ? { protocol: "ws", host, port: 5174 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    outDir: resolve(import.meta.dirname, "dist"),
    emptyOutDir: true,
    sourcemap: true,
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
});
