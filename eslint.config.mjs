import eslint from "@eslint/js";
import tseslint from "typescript-eslint";
import jsdoc from "eslint-plugin-jsdoc";
import svelteParser from "svelte-eslint-parser";

/**
 * Rust-style `///` doc comments read as ordinary line comments in TypeScript,
 * so a doc block written that way is invisible to every tool that consumes
 * JSDoc. The backend documents its items with `///`, which is how the two
 * conventions drift into each other's files.
 *
 * @type {import("eslint").Rule.RuleModule}
 */
const noRustDocComments = {
  meta: {
    type: "problem",
    docs: { description: "Disallow Rust-style `///` doc comments" },
    messages: { rustDoc: "Use a /** … */ JSDoc block, not Rust-style `///`." },
    schema: [],
  },
  create(context) {
    return {
      Program() {
        for (const comment of context.sourceCode.getAllComments()) {
          if (comment.type === "Line" && comment.value.startsWith("/")) {
            context.report({ node: comment, messageId: "rustDoc" });
          }
        }
      },
    };
  },
};

/** @type {import("eslint").ESLint.Plugin} */
const local = { rules: { "no-rust-doc-comments": noRustDocComments } };

export default tseslint.config(
  {
    ignores: [
      "coverage/**",
      "dist/**",
      "e2e-results/**",
      "node_modules/**",
      "src-tauri/target/**",
    ],
  },
  {
    files: ["**/*.{js,mjs,cjs,ts,mts}"],
    extends: [eslint.configs.recommended, ...tseslint.configs.recommended],
  },
  {
    // Comment style, for TypeScript and for a component's `<script>` block.
    //
    // The baseline is every rule in the plugin's four TypeScript categories,
    // taken whole rather than listed by hand: a rule the plugin adds to a
    // category then arrives already enabled on the next upgrade, instead of
    // waiting for somebody to notice it exists. Everything below is a
    // deliberate loosening, and each one says why. See AGENTS.md → Comments.
    files: ["**/*.{ts,mts,svelte}"],
    plugins: { local },
    extends: [
      jsdoc.configs["flat/contents-typescript-error"],
      jsdoc.configs["flat/logical-typescript-error"],
      jsdoc.configs["flat/requirements-typescript-error"],
      jsdoc.configs["flat/stylistic-typescript-error"],
    ],
    rules: {
      "local/no-rust-doc-comments": "error",

      // Not in any category, and the point of the convention: a block that is
      // all tags and no prose says nothing a reader could not already see.
      "jsdoc/require-description": "error",

      // A doc comment here earns its place by saying something the signature
      // does not, so the four rules that demand one per declaration — and a
      // tag per parameter and return — are off. What stays on is the shape of
      // a block somebody did write: `check-param-names` still rejects an
      // `@param` that names no parameter, and `require-param-description`
      // still rejects one with nothing to say.
      "jsdoc/require-jsdoc": "off",
      "jsdoc/require-param": "off",
      "jsdoc/require-returns": "off",

      // `@example` belongs in a doc for a library's callers. Nothing here is
      // consumed outside this repo, and the tests are the worked examples.
      "jsdoc/require-example": "off",
    },
  },
  {
    // Components are linted for comment style alone. The component rules
    // themselves would want eslint-plugin-svelte and a cleanup of their own.
    files: ["**/*.svelte"],
    languageOptions: {
      parser: svelteParser,
      parserOptions: { parser: tseslint.parser },
    },
    rules: {
      // The parser hands the rule a `<script lang="ts">` line as the code
      // preceding a block at the top of the script, so it asks for a blank
      // line there — which prettier then takes straight back out. Every hit
      // in a component was that one case.
      "jsdoc/lines-before-block": "off",
    },
  },
);
