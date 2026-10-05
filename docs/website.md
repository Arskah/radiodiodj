# Website — radiodiodj.org

The project homepage: one static landing page with the feature list and a
download button per platform. It is served by GitHub Pages from the custom
domain `radiodiodj.org`.

It is a page about the app, not the app's manual. The design docs in this folder
stay on GitHub and the page links to them.

## A separate package

`site/` is an [Astro](https://astro.build) project with its **own**
`package.json`, `pnpm-lock.yaml` and `pnpm-workspace.yaml`. It is not a member
of the root workspace, for two reasons:

- **A site change stays under `site/`.** A shared lockfile would put every site
  dependency bump in the root, and `release-please-config.json` excludes the
  `site` path — a commit that only touches it bumps no version, enters no
  changelog and opens no release PR. That only holds while such a commit touches
  nothing else.
- **The app's install does not grow.** Every CI job runs the root
  `pnpm install`; none of them need Astro.

`site/pnpm-workspace.yaml` repeats the root's `minimumReleaseAge`, since a
separate workspace root does not inherit it.

Root tooling still covers the folder: `pnpm lint` and `pnpm format:check` read
`site/**` with the root configs. `.astro` files are checked by `astro check`,
which `pnpm build` in `site/` runs first.

```bash
cd site
pnpm install
pnpm dev        # http://localhost:4321
pnpm build      # astro check, then a static build into site/dist/
```

## What the site takes from the app

Read from the app's own files at build time, so neither can drift:

### Colours

`site/src/lib/theme.ts` imports the two built-in themes,
`src-tauri/themes/midnight.json` and `daylight.json`, and emits their tokens as
CSS custom properties: `midnight` by default, `daylight` under
`prefers-color-scheme: light`. A retinted built-in retints the site on its next
deploy. See [theming.md](./theming.md) for the token contract.

Unlike the app, the site does follow the OS appearance. The app's rule against
repainting unasked protects an operator mid-show; a visitor to a web page has no
show.

### Artwork

The hero vinyl is `src/assets/radiodiodi_disc.svg` with
`radiodiodi_label.svg` on top, the same two parts the app icon and the deck are
composed from. The favicon is `src-tauri/icons/128x128.png`.

### Screenshots

`site/src/assets/screenshots/` holds hand-made captures, shown in this order
when present: `on-air`, `library`, `cue-points` (`.png`, `.jpg` or `.webp`). A
missing file is skipped and the section disappears when there are none, so the
site builds without them.

## Downloads

Download links are **baked at build time**. `site/src/lib/release.ts` asks the
GitHub API for the latest release and maps its assets by file-name suffix:

| button                | asset suffix                 |
| --------------------- | ---------------------------- |
| macOS — Apple Silicon | `_aarch64.dmg`               |
| macOS — Intel         | `_x64.dmg`                   |
| Windows — installer   | `_x64-setup.exe`             |
| Windows — MSI         | `.msi`                       |
| Linux — AppImage      | `.AppImage`                  |
| Linux — Debian        | `.deb`                       |
| Linux — Fedora        | `.rpm`                       |
| GPG signing key       | `radiodiodj-signing-key.asc` |

A bundle the release does not have gets no button, and a platform with no
bundles is left out: `release.yml` uploads whatever built, so a release can be
missing a platform.

When the lookup fails, or the release has no bundles at all:

- **In CI the build fails.** A deploy must not replace working links with none.
  A release still waiting for its bundles is not that case: it is a draft until
  they are uploaded, and GitHub does not report a draft as the latest. See
  [Deploy](#deploy).
- **Locally** the page falls back to one link to the releases page, so the site
  can be worked on offline.

A short inline script moves the visitor's platform first. Both macOS builds stay
listed, because a browser does not report the CPU reliably.

## Updates

The app asks `https://radiodiodj.org/update.json` whether a newer version
exists. It is an endpoint, `site/src/pages/update.json.ts`, built from the same
release lookup as the download links, in the updater's static-manifest format:

```json
{
  "version": "0.26.0",
  "notes": "…the release notes…",
  "pub_date": "2026-10-04T12:00:00Z",
  "platforms": {
    "darwin-aarch64-app": { "url": "…", "signature": "…" }
  }
}
```

**On our domain, not GitHub's.** The URL is compiled into every install and
cannot be changed for one already out there. This one stays ours wherever the
bundles are hosted, and a bad release can be withdrawn by redeploying the site
without it.

| target                  | asset suffix          |
| ----------------------- | --------------------- |
| `darwin-aarch64-app`    | `_aarch64.app.tar.gz` |
| `darwin-x86_64-app`     | `_x64.app.tar.gz`     |
| `windows-x86_64-nsis`   | `_x64-setup.exe`      |
| `windows-x86_64-msi`    | `.msi`                |
| `linux-x86_64-appimage` | `.AppImage`           |
| `linux-x86_64-deb`      | `.deb`                |
| `linux-x86_64-rpm`      | `.rpm`                |

- **A target is listed only when the release has both the bundle and its
  `.sig`**; `signature` is that file's content, fetched at build time. A release
  missing a platform leaves it out.
- **A missing target is an error to the updater, not "no update".** It looks its
  own target up before it compares versions, so an install whose kind is absent
  gets a failed lookup on every check, newer version or not. The app expects
  this and reads it as "nothing here I can install"; the manifest cannot say it
  any other way. The same holds for a release with no signatures at all, whose
  manifest is valid and has no platforms.
- **Every key names its installer.** The updater looks up
  `{os}-{arch}-{installer}` before `{os}-{arch}`; with no bare key, an MSI
  install is never handed the NSIS installer, nor a `.deb` install an AppImage.
- **One bad entry breaks every platform** — the updater validates the whole
  file before it compares versions. So in CI a signature that cannot be fetched
  fails the build rather than publishing a partial manifest. Locally that
  target is left out with a warning, and with GitHub unreachable the manifest
  has no platforms.
- **The signature is baked, the bundle is not.** `url` points at a release
  asset, and running `release.yml` by hand on an existing tag replaces both the
  bundle and its `.sig`. Until the site redeploys — which that run triggers —
  the manifest pairs the new bytes with the old signature, and an install that
  updates in those minutes fails verification and can simply try again. Do not
  rebuild a tag installs are updating to unless it is broken.

How the bundles come to be signed is [signing.md](./signing.md#updater-signing).

## Contact

Two addresses are public: `hello@radiodiodj.org` in the page footer and in
`package.json`, and `security@radiodiodj.org` in [SECURITY.md](../SECURITY.md).

The site also serves `/.well-known/security.txt` (RFC 9116). It is an endpoint,
`site/src/pages/.well-known/security.txt.ts`, not a static file: the format
requires an `Expires` date, and generating it puts that date 300 days past each
deploy instead of leaving it to be bumped by hand. `site.yml` uploads the Pages
artifact with `include-hidden-files`, without which the dot-directory is dropped.

## Deploy

| workflow    | runs on                                                        | does                               |
| ----------- | -------------------------------------------------------------- | ---------------------------------- |
| `site.yml`  | called by `ci.yml` and `pages.yml`                             | installs and builds `site/`        |
| `pages.yml` | a **Release Please** or **Release** run completing, or by hand | calls `site.yml`, deploys to Pages |

The build is in its own workflow because the deploy job needs `pages: write`,
and a called workflow may not ask for more than its caller grants — a pull
request's CI should not hold that permission.

**A release is a draft until it has bundles.** release-please creates the
release when the release PR merges, and `release.yml` uploads the bundles some
minutes later. `"draft": true` in `release-please-config.json` keeps it
unpublished for that stretch, and `upload-release` publishes it straight after
the upload. GitHub's "latest release" skips drafts, so every reader of it — this
site's build in a pull request, a deploy that happens to start mid-release, the
_all files_ link on the live page — keeps seeing the previous release until the
new one can actually be downloaded. Two things follow:

- `"force-tag-creation": true` goes with it and must stay. GitHub creates a
  draft's tag only when it is published, and release-please finds the previous
  release by its tag, so without the setting a push to `main` mid-release would
  not see the release that was just cut.
- A release whose builds all failed uploads nothing and stays a draft, visible
  only to maintainers. The site goes on offering the one before it. Running
  **Release** by hand on the tag builds it again and publishes it.

**The deploy follows Release Please rather than a push.** Baked links are only
right if the build runs after the release's bundles are uploaded, and the
`release: published` event fires before any of them exist: release-please
creates the release first, then `release.yml` builds for half an hour. A Release
Please _workflow run_ contains that build as a called workflow, so it completes
only once `upload-release` has finished. On a push that releases nothing the run
ends in seconds, and the site deploys just the same.

A **Release** run of its own only exists when `release.yml` is started by hand
to rebuild a tag. It replaces the bundles, so the site follows it too.

`release-please.yml` itself is not involved: the site reacts to it.

Two consequences:

- `pages.yml` names the workflows, `workflows: [Release Please, Release]`.
  **Renaming either stops the site deploying after it**, silently.
- A deploy that starts while a release is still building — `pages.yml` run by
  hand, or following another push to `main` — republishes the previous release.
  The release's own run deploys the new one once it is published.

A release that finished with a platform missing still deploys, showing the
platforms that built.
