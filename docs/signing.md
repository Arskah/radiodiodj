# Application signing

Status of code signing across the three release platforms.

| Platform | State                                          | OS warning removed?                                                       |
| -------- | ---------------------------------------------- | ------------------------------------------------------------------------- |
| macOS    | Developer ID + notarization **wired, dormant** | Yes, once the secrets are set. Ad-hoc signing stays the fallback.         |
| Linux    | GPG AppImage signing **wired, dormant**        | N/A — Linux has no Gatekeeper; signature is for manual verification only. |
| Windows  | Authenticode **scaffolded, dormant**           | No — needs a real certificate before SmartScreen goes away.               |

Nothing is active by default: every platform stays inert until its secrets are added. No
secret means the build behaves exactly as before — ad-hoc signed on macOS, unsigned
elsewhere.

---

## macOS — Developer ID + notarization (wired, needs secrets)

Notarization is what actually removes the Gatekeeper warning. It needs a paid Apple
Developer Program membership ($99/yr) and a **Developer ID Application** certificate;
nothing free gets there.

`build.yml` signs and notarizes only when `APPLE_SIGNING_IDENTITY` is present. Without it
the build falls back to the ad-hoc identity pinned in `src-tauri/tauri.conf.json`:

```json
"bundle": { "macOS": { "signingIdentity": "-" } }
```

`"-"` is the ad-hoc identity. It gives the `.app` a valid signature — required for the
binary to launch at all on Apple Silicon — but does **not** notarize it, so a downloaded
`.dmg` stays quarantined and first launch shows "unidentified developer". Users get past
it once via **System Settings → Privacy & Security → Open Anyway**, or with:

```sh
xattr -dr com.apple.quarantine /Applications/RadiodioDJ.app
```

The ad-hoc value has to stay in the config file so local and unconfigured builds keep it.
Because a `signingIdentity` in the config outranks the `APPLE_SIGNING_IDENTITY` env var,
the workflow writes a generated `src-tauri/signing.macos.conf.json` overlay (gitignored)
and passes it as `--config`. The bundler imports `APPLE_CERTIFICATE` into a temporary
keychain on its own — no separate keychain action is needed.

No entitlements file: hardened runtime is Tauri's default, the app is not sandboxed
(Developer ID distribution never is), and `cpal` is output-only, so there is no
microphone usage description to declare.

Notarization credentials reach the bundler through one route or the other, never both.
The bundler picks by _presence_: if `APPLE_ID`, `APPLE_PASSWORD` and `APPLE_TEAM_ID` are
all defined it uses them, otherwise it uses the API key. A GitHub expression for a secret
that does not exist still defines the variable as an empty string, so the workflow stages
every credential under an `APPLE_*_IN` name and exports only the route it actually has.
Do not add plain `APPLE_*` entries back to the job `env` block.

### Activate

1. Create the certificate (do this yourself — the private key must never come from an
   agent or land in the repo). Xcode → Settings → Accounts → **Manage Certificates** →
   **+** → **Developer ID Application**. Then confirm it:

   ```sh
   security find-identity -v -p codesigning
   ```

   The full `Developer ID Application: Name (TEAMID)` string is the signing identity;
   `TEAMID` is the team ID. "Apple Development" and "Mac App Distribution" certificates
   do not work for direct `.dmg` distribution.

2. Export it from Keychain Access (right-click the private key → Export → `.p12`, set a
   password) and encode it:

   ```sh
   base64 -i certificate.p12 | pbcopy
   ```

   Delete the `.p12` afterwards.

3. Create an App Store Connect API key for notarization: appstoreconnect.apple.com →
   Users and Access → Integrations → Keys → **+**, role **Developer**. The `.p8`
   downloads exactly once. Encode it the same way: `base64 -i AuthKey_XXXX.p8 | pbcopy`.

4. Add repo secrets (Settings → Secrets and variables → Actions):

   | Secret                       | Value                                     |
   | ---------------------------- | ----------------------------------------- |
   | `APPLE_CERTIFICATE`          | base64 of the `.p12`                      |
   | `APPLE_CERTIFICATE_PASSWORD` | the `.p12` export password                |
   | `APPLE_SIGNING_IDENTITY`     | `Developer ID Application: Name (TEAMID)` |
   | `APPLE_API_KEY`              | the Key ID column value                   |
   | `APPLE_API_ISSUER`           | the issuer UUID above the keys table      |
   | `APPLE_API_KEY_P8`           | base64 of the `.p8`                       |

   The workflow decodes `APPLE_API_KEY_P8` into `$RUNNER_TEMP` and points
   `APPLE_API_KEY_PATH` at it.

   Alternative to the API key: set `APPLE_ID` (account email), `APPLE_PASSWORD` (an
   [app-specific password]) and `APPLE_TEAM_ID` instead of the three `APPLE_API_*`
   secrets. Both routes are already read as env by `build.yml`; the API key is preferred
   because it survives Apple ID password changes.

Next release signs and notarizes both macOS bundles automatically. No workflow edit
needed. Notarization adds roughly 2–10 minutes per architecture.

### Verify a release build

Download the `.dmg` from the release on a machine that has never held the certificate —
the quarantine attribute only exists on real downloads.

```sh
codesign -dv --verbose=4 RadiodioDJ.app     # Authority: Developer ID Application…, flags=runtime
codesign --verify --deep --strict RadiodioDJ.app
spctl -a -vvv -t install RadiodioDJ.app     # source=Notarized Developer ID
xcrun stapler validate RadiodioDJ.app
```

A notarization failure reports a submission ID; read the reason with
`xcrun notarytool log <id> --key <p8> --key-id <APPLE_API_KEY> --issuer <APPLE_API_ISSUER>`.
The usual causes are an unsigned nested binary or a missing hardened runtime.

The certificate expires in five years. Builds notarized before then keep validating,
because the signature is timestamped.

[app-specific password]: https://support.apple.com/en-ca/HT204397

---

## Linux — GPG AppImage signing (wired, needs secrets)

The build workflow imports a GPG key and enables signing only when `GPG_PRIVATE_KEY` is
present. Signature covers the AppImage target; users verify it manually with the AppImage
validate tool, so it only adds value if you publish the key ID on a trusted channel.

### Activate

1. Generate a signing key (do this yourself — the private key must never come from an
   agent or land in the repo):

   ```sh
   gpg --full-generate-key            # pick RSA 4096, set a passphrase
   gpg --list-secret-keys --keyid-format=long   # note the long key ID
   gpg --armor --export-secret-keys <KEY_ID> > private.asc
   ```

2. Add repo secrets (Settings → Secrets and variables → Actions):

   | Secret            | Value                                                         |
   | ----------------- | ------------------------------------------------------------- |
   | `GPG_PRIVATE_KEY` | contents of `private.asc`                                     |
   | `GPG_KEY_ID`      | the long key ID (optional; picks the key if you hold several) |
   | `GPG_PASSPHRASE`  | the key passphrase (maps to `APPIMAGETOOL_SIGN_PASSPHRASE`)   |

3. Delete `private.asc` locally and publish the **public** key + key ID somewhere
   authenticated (README / site) so users can verify.

Next release signs the AppImage automatically. No workflow edit needed.

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
