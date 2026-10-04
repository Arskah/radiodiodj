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
- **Locally** the page falls back to one link to the releases page, so the site
  can be worked on offline.

A short inline script moves the visitor's platform first. Both macOS builds stay
listed, because a browser does not report the CPU reliably.

## Contact

Two addresses are public: `hello@radiodiodj.org` in the page footer and in
`package.json`, and `security@radiodiodj.org` in [SECURITY.md](../SECURITY.md).

The site also serves `/.well-known/security.txt` (RFC 9116). It is an endpoint,
`site/src/pages/.well-known/security.txt.ts`, not a static file: the format
requires an `Expires` date, and generating it puts that date 300 days past each
deploy instead of leaving it to be bumped by hand. `site.yml` uploads the Pages
artifact with `include-hidden-files`, without which the dot-directory is dropped.

## Deploy

| workflow    | runs on                                         | does                               |
| ----------- | ----------------------------------------------- | ---------------------------------- |
| `site.yml`  | called by `ci.yml` and `pages.yml`              | installs and builds `site/`        |
| `pages.yml` | a **Release Please** run completing, or by hand | calls `site.yml`, deploys to Pages |

The build is in its own workflow because the deploy job needs `pages: write`,
and a called workflow may not ask for more than its caller grants — a pull
request's CI should not hold that permission.

**The deploy follows Release Please rather than a push.** Baked links are only
right if the build runs after the release's bundles are uploaded, and the
`release: published` event fires before any of them exist: release-please
publishes the tag first, then `release.yml` builds for half an hour. A Release
Please _workflow run_ contains that build as a called workflow, so it completes
only once `upload-release` has finished. On a push that releases nothing the run
ends in seconds, and the site deploys just the same.

`release-please.yml` itself is not involved: the site reacts to it.

Two consequences:

- `pages.yml` names the workflow, `workflows: [Release Please]`. **Renaming
  that workflow stops the site deploying**, silently.
- Running `pages.yml` by hand while a release is still building fails the
  build — the latest release has no bundles yet. Run it again afterwards.

A release that finished with a platform missing still deploys, showing the
platforms that built.
