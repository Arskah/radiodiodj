import eslint from "@eslint/js";
import tseslint from "typescript-eslint";

export default tseslint.config(
  {
    ignores: [
      ".svelte-check/**",
      "coverage/**",
      "dist/**",
      "e2e-results/**",
      "node_modules/**",
      "src-tauri/target/**",
    ],
  },
  eslint.configs.recommended,
  ...tseslint.configs.recommended,
);
