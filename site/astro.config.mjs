import { defineConfig } from "astro/config";

// See `docs/website.md`.
export default defineConfig({
  site: "https://radiodiodj.org",
  vite: {
    server: {
      fs: { allow: [".."] },
    },
  },
});
