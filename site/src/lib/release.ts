const REPO = "Arskah/radiodiodj";

/** Where a visitor picks a bundle by hand when no direct link is known. */
export const RELEASES_URL = `https://github.com/${REPO}/releases/latest`;

/** One downloadable bundle. */
export interface Download {
  label: string;
  href: string;
}

/** The bundles built for one operating system. */
export interface Platform {
  id: "macos" | "windows" | "linux";
  name: string;
  note: string;
  downloads: Download[];
}

/** The latest published release, reduced to what the page links to. */
export interface LatestRelease {
  version: string;
  platforms: Platform[];
  signingKey: string | undefined;
}

interface Asset {
  name: string;
  browser_download_url: string;
}

interface Release {
  tag_name: string;
  body: string | null;
  published_at: string;
  assets: Asset[];
}

/** One bundle an install can update itself from. */
export interface UpdateTarget {
  url: string;
  signature: string;
}

/**
 * The static manifest the app's updater reads. The field names are the
 * updater's, not ours. See `docs/website.md#updates`.
 */
export interface UpdateManifest {
  version: string;
  notes: string;
  pub_date?: string;
  platforms: Record<string, UpdateTarget>;
}

const SIGNING_KEY = "radiodiodj-signing-key.asc";

/** Updater target to asset suffix. See `docs/website.md#updates`. */
const UPDATE_TARGETS: [target: string, suffix: string][] = [
  ["darwin-aarch64-app", "_aarch64.app.tar.gz"],
  ["darwin-x86_64-app", "_x64.app.tar.gz"],
  ["windows-x86_64-nsis", "_x64-setup.exe"],
  ["windows-x86_64-msi", ".msi"],
  ["linux-x86_64-appimage", ".AppImage"],
  ["linux-x86_64-deb", ".deb"],
  ["linux-x86_64-rpm", ".rpm"],
];

/** Asset suffix to button, per platform. See `docs/website.md#downloads`. */
const PLATFORMS: {
  id: Platform["id"];
  name: string;
  note: string;
  bundles: [suffix: string, label: string][];
}[] = [
  {
    id: "macos",
    name: "macOS",
    note: "Signed and notarized.",
    bundles: [
      ["_aarch64.dmg", "Apple Silicon (.dmg)"],
      ["_x64.dmg", "Intel (.dmg)"],
    ],
  },
  {
    id: "windows",
    name: "Windows",
    note: "Not code-signed yet, so SmartScreen will warn on first run.",
    bundles: [
      ["_x64-setup.exe", "Installer (.exe)"],
      [".msi", "MSI package (.msi)"],
    ],
  },
  {
    id: "linux",
    name: "Linux",
    note: "x86-64. The AppImage is GPG-signed.",
    bundles: [
      [".AppImage", "AppImage"],
      [".deb", "Debian / Ubuntu (.deb)"],
      [".rpm", "Fedora / RHEL (.rpm)"],
    ],
  },
];

let lookup: Promise<Release | undefined> | undefined;

/**
 * Asks GitHub for the latest release, once per build however many pages want
 * it.
 * @returns The release, or `undefined` when GitHub could not be reached
 * outside CI.
 * @throws {Error} In CI, when the lookup fails — a deploy must not replace a
 *   working page with an empty one.
 */
function release(): Promise<Release | undefined> {
  lookup ??= (async () => {
    try {
      const token = process.env.GITHUB_TOKEN;
      const response = await fetch(
        `https://api.github.com/repos/${REPO}/releases/latest`,
        {
          headers: {
            Accept: "application/vnd.github+json",
            ...(token ? { Authorization: `Bearer ${token}` } : {}),
          },
        },
      );
      if (!response.ok) {
        throw new Error(`GitHub answered ${response.status}`);
      }
      return (await response.json()) as Release;
    } catch (error) {
      if (process.env.CI) {
        throw new Error("Latest release lookup failed", { cause: error });
      }
      console.warn("Latest release lookup failed.", error);
      return undefined;
    }
  })();
  return lookup;
}

function versionOf(found: Release): string {
  return found.tag_name.replace(/^v/, "");
}

/**
 * The latest release as the page links to it. A platform whose bundles are
 * missing from the release is left out rather than linked dead.
 * @returns The release, or `undefined` when GitHub could not be reached
 * outside CI.
 * @throws {Error} In CI, when the lookup fails or the release has no bundles.
 */
export async function latestRelease(): Promise<LatestRelease | undefined> {
  const found = await release();
  if (!found) return undefined;

  const platforms = PLATFORMS.map(({ bundles, ...platform }) => ({
    ...platform,
    downloads: bundles.flatMap(([suffix, label]) => {
      const asset = found.assets.find((a) => a.name.endsWith(suffix));
      return asset ? [{ label, href: asset.browser_download_url }] : [];
    }),
  })).filter((platform) => platform.downloads.length > 0);
  if (platforms.length === 0) {
    if (process.env.CI) throw new Error(`${found.tag_name} has no bundles`);
    return undefined;
  }

  return {
    version: versionOf(found),
    platforms,
    signingKey: found.assets.find((a) => a.name === SIGNING_KEY)
      ?.browser_download_url,
  };
}

/**
 * Pairs each updater target with its bundle and that bundle's `.sig`. A target
 * missing either is left out, so an install of that kind is offered nothing
 * rather than something it cannot verify.
 * @param assets The release's assets.
 * @returns Per target, the bundle's URL and the URL of its signature.
 */
export function updateTargets(
  assets: Asset[],
): { target: string; url: string; signatureUrl: string }[] {
  return UPDATE_TARGETS.flatMap(([target, suffix]) => {
    const bundle = assets.find((a) => a.name.endsWith(suffix));
    const signature =
      bundle && assets.find((a) => a.name === `${bundle.name}.sig`);
    return bundle && signature
      ? [
          {
            target,
            url: bundle.browser_download_url,
            signatureUrl: signature.browser_download_url,
          },
        ]
      : [];
  });
}

/**
 * The latest release as the app's updater reads it.
 * @returns The manifest. With no platforms when the release carries no
 * signatures, or when GitHub could not be reached outside CI.
 * @throws {Error} In CI, when the lookup or a signature download fails. The
 *   updater rejects the whole manifest over one bad entry, so a partial one is
 *   never published.
 */
export async function updateManifest(): Promise<UpdateManifest> {
  const found = await release();
  if (!found) return { version: "0.0.0", notes: "", platforms: {} };

  const platforms: UpdateManifest["platforms"] = {};
  for (const { target, url, signatureUrl } of updateTargets(found.assets)) {
    const response = await fetch(signatureUrl);
    const signature = response.ok ? (await response.text()).trim() : "";
    if (!signature) {
      throw new Error(`No signature for ${target} at ${signatureUrl}`);
    }
    platforms[target] = { url, signature };
  }

  return {
    version: versionOf(found),
    notes: found.body ?? "",
    pub_date: found.published_at,
    platforms,
  };
}
