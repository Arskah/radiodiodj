/**
 * Writes `THIRD-PARTY-NOTICES.md`: what a RadiodioDJ bundle carries that is not
 * RadiodioDJ, and the licence each part comes under — and beside it
 * `THIRD-PARTY-LICENSES.txt`, the licence texts of the Rust crates. Run with
 * `pnpm notices`; `pnpm notices --check` fails instead of writing when either
 * file is stale.
 */
import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";

import { format, resolveConfig } from "prettier";

const ROOT = resolve(import.meta.dirname, "..");
const NOTICES = join(ROOT, "THIRD-PARTY-NOTICES.md");
const LICENCES = join(ROOT, "THIRD-PARTY-LICENSES.txt");
const MANIFEST = join(ROOT, "src-tauri", "Cargo.toml");
const OWN_CRATE = "radiodiodj";

/**
 * Packages whose licence text is reproduced: a font or an icon set travels as
 * the work itself, and the OFL and Apache licences both ask that a copy goes
 * with it.
 */
const ASSETS: Record<string, string> = {
  "@fontsource/inter": "Inter",
  "@fontsource/jetbrains-mono": "JetBrains Mono",
  "material-symbols": "Material Symbols",
};

/** A dev dependency, but its runtime is compiled into the renderer bundle. */
const BUNDLED_DEV_DEPENDENCIES = ["svelte"];

/**
 * Code-point order, not `localeCompare`: the output is compared byte for byte
 * in CI, and collation follows the machine's locale.
 */
function compare(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

interface Component {
  name: string;
  licence: string;
  source: string;
}

interface PackageJson {
  dependencies?: Record<string, string>;
  license?: string;
  homepage?: string;
  repository?: string | { url?: string };
}

function readPackage(dir: string): PackageJson {
  return JSON.parse(
    readFileSync(join(dir, "package.json"), "utf8"),
  ) as PackageJson;
}

function packageDir(name: string): string {
  return join(ROOT, "node_modules", name);
}

function sourceUrl(pkg: PackageJson, name: string): string {
  const declared =
    typeof pkg.repository === "string" ? pkg.repository : pkg.repository?.url;
  const url = (declared ?? pkg.homepage ?? "")
    .replace(/^git\+/, "")
    .replace(/\.git$/, "");
  return url.startsWith("https://")
    ? url
    : `https://www.npmjs.com/package/${name}`;
}

function npmComponent(name: string): Component {
  const pkg = readPackage(packageDir(name));
  if (!pkg.license) throw new Error(`${name} declares no licence`);
  return { name, licence: pkg.license, source: sourceUrl(pkg, name) };
}

function licenceText(name: string): string {
  const file = ["LICENSE", "LICENSE.md", "LICENSE.txt"]
    .map((candidate) => join(packageDir(name), candidate))
    .find((candidate) => existsSync(candidate));
  if (!file) throw new Error(`${name} ships no licence file`);
  return readFileSync(file, "utf8").trim();
}

interface CargoPackage {
  name: string;
  version: string;
  license: string | null;
  repository: string | null;
  manifest_path: string;
}

interface Crate extends Component {
  /** Where cargo unpacked the crate, which is where its licence files are. */
  dir: string;
}

function cargo(args: string[]): string {
  return execFileSync(
    "cargo",
    [args[0]!, "--manifest-path", MANIFEST, "--locked", ...args.slice(1)],
    { encoding: "utf8", maxBuffer: 256 * 1024 * 1024 },
  );
}

/**
 * Every crate linked into the binary on any platform: normal dependencies
 * only, so neither a build script's nor a test's. `cargo tree` knows which
 * crates those are and `cargo metadata` knows where each one is on disk.
 */
function crates(): Crate[] {
  const metadata = JSON.parse(cargo(["metadata", "--format-version", "1"])) as {
    packages: CargoPackage[];
  };
  const packages = new Map(
    metadata.packages.map((pkg) => [`${pkg.name} v${pkg.version}`, pkg]),
  );

  const tree = cargo([
    "tree",
    "--edges",
    "normal",
    "--target",
    "all",
    "--prefix",
    "none",
    "--format",
    "{p}",
  ]);
  const linked = new Set(
    tree
      .split("\n")
      .map((line) => line.split(" ").slice(0, 2).join(" "))
      .filter((id) => id && !id.startsWith(`${OWN_CRATE} `)),
  );

  return [...linked]
    .map((id) => {
      const pkg = packages.get(id);
      if (!pkg) throw new Error(`cargo metadata does not know ${id}`);
      if (!pkg.license) throw new Error(`crate ${id} declares no licence`);
      return {
        name: pkg.name,
        licence: pkg.license,
        source: pkg.repository ?? `https://crates.io/crates/${pkg.name}`,
        dir: dirname(pkg.manifest_path),
      };
    })
    .sort(
      (a, b) =>
        compare(a.name, b.name) ||
        compare(a.licence, b.licence) ||
        compare(a.dir, b.dir),
    );
}

/**
 * Keyed without the version: two versions of one crate under one licence are
 * one row, and a bump that changes nothing else leaves the file alone.
 */
function distinct(components: Component[]): Component[] {
  const seen = new Map(components.map((c) => [`${c.name}|${c.licence}`, c]));
  return [...seen.values()];
}

/** `LICENSE.spdx` is a machine-readable tag, not a text anyone was asked to keep. */
const LICENCE_FILE = /^(licen[sc]e|copying|copyright|notice|unlicense)/i;
const NOT_A_TEXT = /\.spdx$/i;
const RULE = "=".repeat(80);

/**
 * The licence texts the crates ship, each printed once however many crates
 * carry it: most Apache-2.0 files are byte-identical, most MIT files differ
 * by a copyright line.
 */
function licenceTexts(linked: Crate[]): string {
  const users = new Map<string, Set<string>>();
  const silent = new Set<string>();
  for (const crate of linked) {
    const files = readdirSync(crate.dir, { withFileTypes: true })
      .filter(
        (entry) =>
          entry.isFile() &&
          LICENCE_FILE.test(entry.name) &&
          !NOT_A_TEXT.test(entry.name),
      )
      .map((entry) => entry.name)
      .sort(compare);
    if (files.length === 0) silent.add(`${crate.name} (${crate.licence})`);
    for (const file of files) {
      const text = readFileSync(join(crate.dir, file), "utf8")
        .replace(/\r\n/g, "\n")
        .replace(/[ \t]+$/gm, "")
        .trim();
      if (!text) continue;
      const names = users.get(text) ?? new Set<string>();
      names.add(crate.name);
      users.set(text, names);
    }
  }

  const sections = [...users]
    .map(([text, names]) => ({
      names: [...names].sort(compare).join(", "),
      text,
    }))
    .sort((a, b) => compare(a.names, b.names) || compare(a.text, b.text))
    .map(({ names, text }) => [RULE, names, RULE, "", text].join("\n"));

  return [
    "Licence texts of the Rust crates linked into RadiodioDJ.",
    "Generated by `pnpm notices`. Do not edit by hand.",
    "",
    "Each text is printed once, under the crates that ship it. A crate offered",
    "under a choice of licences ships one text per licence. The crates and their",
    "sources are listed in THIRD-PARTY-NOTICES.md.",
    "",
    "These crates ship no licence file; each is under the licence it declares:",
    "",
    ...[...silent].sort(compare).map((name) => `  ${name}`),
    "",
    sections.join("\n\n"),
    "",
  ].join("\n");
}

function table(components: Component[]): string {
  const rows = components.map(
    (c) => `| ${c.name} | ${c.licence} | <${c.source}> |`,
  );
  return [
    "| Component | Licence | Source |",
    "| --- | --- | --- |",
    ...rows,
  ].join("\n");
}

function assetSection(name: string): string {
  const component = npmComponent(name);
  return [
    `### ${ASSETS[name]}`,
    `${component.licence} — <${component.source}>`,
    ["```text", licenceText(name), "```"].join("\n"),
  ].join("\n\n");
}

async function renderNotices(linked: Crate[]): Promise<string> {
  const dependencies = Object.keys(readPackage(ROOT).dependencies ?? {}).sort(
    compare,
  );
  const libraries = [...dependencies, ...BUNDLED_DEV_DEPENDENCIES]
    .filter((name) => !(name in ASSETS))
    .sort(compare)
    .map(npmComponent);

  const markdown = [
    "# Third-party notices",
    "RadiodioDJ is licensed under the [GNU General Public License v3.0 or later](LICENSE). " +
      "A RadiodioDJ bundle also carries the components below, each under its own licence. " +
      "Where a component offers a choice of licences, RadiodioDJ takes it under one compatible with the GPL.",
    "This file is generated by `pnpm notices` from `package.json` and `src-tauri/Cargo.lock`. Do not edit it by hand.",
    "## Fonts and icons",
    ...Object.keys(ASSETS).map(assetSection),
    "## Renderer libraries",
    table(libraries),
    "## Rust crates",
    "Every crate linked into the application on macOS, Windows or Linux. " +
      "Their licence texts are in [THIRD-PARTY-LICENSES.txt](THIRD-PARTY-LICENSES.txt).",
    table(distinct(linked)),
  ].join("\n\n");

  return format(markdown, {
    ...(await resolveConfig(NOTICES)),
    filepath: NOTICES,
  });
}

const linked = crates();
const outputs: [string, string][] = [
  [NOTICES, await renderNotices(linked)],
  [LICENCES, licenceTexts(linked)],
];

if (process.argv.includes("--check")) {
  const stale = outputs
    .filter(
      ([path, rendered]) =>
        !existsSync(path) || readFileSync(path, "utf8") !== rendered,
    )
    .map(([path]) => basename(path));
  if (stale.length > 0) {
    console.error(
      `${stale.join(" and ")} stale. Run \`pnpm notices\` and commit the result.`,
    );
    process.exit(1);
  }
} else {
  for (const [path, rendered] of outputs) writeFileSync(path, rendered);
}
