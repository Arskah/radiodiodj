# Application signing

Status of code signing across the three release platforms.

| Platform | State                                | OS warning removed?                                                       |
| -------- | ------------------------------------ | ------------------------------------------------------------------------- |
| macOS    | Developer ID + notarization **live** | Yes — releases are notarized and stapled since v0.21.1.                   |
| Linux    | GPG AppImage signing **live**        | N/A — Linux has no Gatekeeper; signature is for manual verification only. |
| Windows  | Authenticode **scaffolded, dormant** | No — needs a real certificate before SmartScreen goes away.               |

macOS and Linux signing are configured and run on every release; their secrets are set in
the repo and no workflow edit is needed. The setup steps below are kept as the reference
for rotating or re-creating those credentials. Windows is the only platform still inert —
it needs a certificate before anything signs.

Every platform is gated on the presence of its secrets, so a fork or a repo without them
builds exactly as before: ad-hoc signed on macOS, unsigned elsewhere.

---

## macOS — Developer ID + notarization (live)

Notarized through the **App Store Connect API key** route: `APPLE_API_KEY`,
`APPLE_API_ISSUER`, `APPLE_API_KEY_P8`. `APPLE_TEAM_ID` is set but unread — it only counts
on the Apple ID route. Adds 2–10 min per architecture.

Mechanics worth knowing before touching `build.yml`:

- **Overlay, not env.** A `signingIdentity` in `tauri.conf.json` outranks
  `APPLE_SIGNING_IDENTITY`, so the workflow generates `src-tauri/signing.macos.conf.json`
  (gitignored) and passes it as `--config`.
- **Ad-hoc fallback.** `"signingIdentity": "-"` stays in `tauri.conf.json` for local and
  unconfigured builds: valid signature, required to launch on Apple Silicon, not
  notarized. Past the resulting quarantine via Privacy & Security → **Open Anyway**, or
  `xattr -dr com.apple.quarantine /Applications/RadiodioDJ.app`.
- **One notarization route, never both.** The bundler picks by _presence_: `APPLE_ID` +
  `APPLE_PASSWORD` + `APPLE_TEAM_ID` if all three are defined, otherwise the API key. An
  absent GitHub secret still defines an empty variable, so credentials are staged as
  `APPLE_*_IN` and only the live route is exported. Do not add plain `APPLE_*` entries
  back to the job `env` block.
- **No keychain step, no entitlements.** The bundler imports `APPLE_CERTIFICATE` into a
  temporary keychain itself. Hardened runtime is Tauri's default, the app is unsandboxed,
  and `cpal` is output-only.

### Rotate

The certificate lasts five years; builds signed before expiry keep validating because the
signature is timestamped. Create the key material yourself — it must never come from an
agent or land in the repo.

1. Xcode → Settings → Accounts → **Manage Certificates** → **+** → **Developer ID
   Application**. `security find-identity -v -p codesigning` prints the
   `Developer ID Application: Name (TEAMID)` string — that is the signing identity.
   "Apple Development" and "Mac App Distribution" certificates do not work for `.dmg`
   distribution.
2. Keychain Access → right-click the private key → Export as `.p12` with a password →
   `base64 -i certificate.p12 | pbcopy`. Delete the `.p12`.
3. appstoreconnect.apple.com → Users and Access → Integrations → Keys → **+**, role
   **Developer**. The `.p8` downloads exactly once: `base64 -i AuthKey_XXXX.p8 | pbcopy`.
4. Replace the secrets (Settings → Secrets and variables → Actions):

   | Secret                       | Value                                     |
   | ---------------------------- | ----------------------------------------- |
   | `APPLE_CERTIFICATE`          | base64 of the `.p12`                      |
   | `APPLE_CERTIFICATE_PASSWORD` | the `.p12` export password                |
   | `APPLE_SIGNING_IDENTITY`     | `Developer ID Application: Name (TEAMID)` |
   | `APPLE_API_KEY`              | the Key ID column value                   |
   | `APPLE_API_ISSUER`           | the issuer UUID above the keys table      |
   | `APPLE_API_KEY_P8`           | base64 of the `.p8`                       |

   The workflow decodes `APPLE_API_KEY_P8` into `$RUNNER_TEMP` and points
   `APPLE_API_KEY_PATH` at it. The alternative is `APPLE_ID` + `APPLE_PASSWORD` (an
   [app-specific password]) + `APPLE_TEAM_ID`; the API key survives Apple ID password
   changes.

### Verify a release build

Download the `.dmg` on a machine that has never held the certificate — the quarantine
attribute only exists on real downloads.

```sh
codesign -dv --verbose=4 RadiodioDJ.app     # Authority: Developer ID Application…, flags=runtime
codesign --verify --deep --strict RadiodioDJ.app
spctl -a -vvv -t install RadiodioDJ.app     # source=Notarized Developer ID
xcrun stapler validate RadiodioDJ.app
```

A notarization failure reports a submission ID:
`xcrun notarytool log <id> --key <p8> --key-id <APPLE_API_KEY> --issuer <APPLE_API_ISSUER>`.
Usual causes: an unsigned nested binary, or a missing hardened runtime.

[app-specific password]: https://support.apple.com/en-ca/HT204397

---

## Linux — GPG AppImage signing (live)

`build.yml` imports `GPG_PRIVATE_KEY` and signs the AppImage target only; verification is
manual, via the AppImage validate tool. `release.yml` writes the `GPG_PUBLIC_KEY` repo
**variable** (not a secret) out as `radiodiodj-signing-key.asc` and uploads it with the
bundles.

### Rotate

Generate the key yourself — the private key must never come from an agent or land in the
repo.

```sh
gpg --full-generate-key                      # RSA 4096, set a passphrase
gpg --list-secret-keys --keyid-format=long   # note the long key ID
gpg --armor --export-secret-keys <KEY_ID> > private.asc
```

Replace the secrets, then delete `private.asc` and update the `GPG_PUBLIC_KEY` variable
with `gpg --armor --export <KEY_ID>` so releases ship the matching public key.

| Secret            | Value                                                         |
| ----------------- | ------------------------------------------------------------- |
| `GPG_PRIVATE_KEY` | contents of `private.asc`                                     |
| `GPG_KEY_ID`      | the long key ID (optional; picks the key if you hold several) |
| `GPG_PASSPHRASE`  | the key passphrase (maps to `APPIMAGETOOL_SIGN_PASSPHRASE`)   |

---

## Windows — Authenticode (scaffolded, needs a certificate)

There is **no free way** to remove the SmartScreen warning — it requires a real
code-signing certificate. Self-signed certs do not help (still warn, worse UX).

The signing hook is `src-tauri/signing.windows.conf.json`, applied as a Tauri `--config`
overlay only when the release workflow's `sign-windows` input is `true`. It currently
targets **Azure Trusted Signing** via [`trusted-signing-cli`].

### Certificate options

- **SignPath Foundation** — free for approved OSS projects, and the repo is
  `GPL-3.0-or-later`, so it qualifies. SignPath signs artifacts **after** the build via
  its own GitHub Action, so it does _not_ use the `signCommand` overlay — it needs a
  post-build signing step and the upload job has to take the signed files rather than the
  raw `staging/` upload. Approval is an application with a review queue; it signs only
  public CI builds.
- **Azure Trusted Signing** — ~$10/mo, individual identity now allowed (needs a few years
  of verifiable history). Certificate stays in Azure, never on the runner, and signatures
  are timestamped for you. Works with the scaffolded `signCommand` below.
- **OV certificate** — $100–400/yr from a CA. Since June 2023 the private key must live on
  a hardware token or HSM, so CI signing needs a CA that offers a cloud HSM. Configure with
  `certificateThumbprint` + `digestAlgorithm` + `timestampUrl` under `bundle.windows`
  instead of `signCommand`. It buys a real publisher identity but not instant SmartScreen
  trust — reputation still accrues over downloads (the old EV shortcut is gone).

### Activate (Azure Trusted Signing route)

1. Set up a Trusted Signing account + certificate profile in Azure and a service principal.
2. Edit `src-tauri/signing.windows.conf.json` — replace the endpoint, account
   (`REPLACE_WITH_TRUSTED_SIGNING_ACCOUNT`), and cert profile
   (`REPLACE_WITH_CERTIFICATE_PROFILE`).
3. Add repo secrets: `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`
   (already read as env by the build workflow).
4. Add a step to `build.yml` that installs `trusted-signing-cli` on the Windows runner
   (`cargo install trusted-signing-cli`). **This step does not exist yet** — flipping the
   flag without it fails the build with `cmd not found`.
5. Set `sign-windows: true` on the `build-windows-x64` job in
   `.github/workflows/release.yml`.

### Activate (SignPath route)

Not scaffolded — this route bypasses `signing.windows.conf.json` entirely. Leave
`sign-windows: false` and instead:

1. Apply to the [SignPath Foundation] OSS programme; get the project, signing policy and
   CI user approved.
2. Add a signing step after the Windows build that submits the unsigned `.msi` / `.exe`
   to SignPath and writes the signed artifacts back over the staged ones, so the existing
   upload job picks them up.
3. Add the `SIGNPATH_API_TOKEN` secret and the organization / project / policy slugs.

[SignPath Foundation]: https://about.signpath.io/product/open-source
[`trusted-signing-cli`]: https://github.com/Levminer/trusted-signing-cli
