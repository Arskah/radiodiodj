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

const SIGNING_KEY = "radiodiodj-signing-key.asc";

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

/**
 * Looks up the latest release at build time. A platform whose bundles are
 * missing from the release is left out rather than linked dead.
 * @returns The release, or `undefined` when GitHub could not be reached
 * outside CI.
 * @throws {Error} In CI, when the lookup fails or the release has no bundles —
 *   a deploy must not replace working links with none.
 */
export async function latestRelease(): Promise<LatestRelease | undefined> {
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
    const release = (await response.json()) as {
      tag_name: string;
      assets: Asset[];
    };

    const platforms = PLATFORMS.map(({ bundles, ...platform }) => ({
      ...platform,
      downloads: bundles.flatMap(([suffix, label]) => {
        const asset = release.assets.find((a) => a.name.endsWith(suffix));
        return asset ? [{ label, href: asset.browser_download_url }] : [];
      }),
    })).filter((platform) => platform.downloads.length > 0);
    if (platforms.length === 0) {
      throw new Error(`${release.tag_name} has no bundles`);
    }

    return {
      version: release.tag_name.replace(/^v/, ""),
      platforms,
      signingKey: release.assets.find((a) => a.name === SIGNING_KEY)
        ?.browser_download_url,
    };
  } catch (error) {
    if (process.env.CI) {
      throw new Error("Latest release lookup failed", { cause: error });
    }
    console.warn("Latest release lookup failed; linking to GitHub.", error);
    return undefined;
  }
}
