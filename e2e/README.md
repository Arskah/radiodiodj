# e2e

End-to-end tests for RadiodioDJ via `tauri-driver` + WebdriverIO.

## Platform support

**Linux only.** macOS WKWebView ships no WebDriver implementation, and Tauri's `tauri-driver` does not support darwin. Mac developers have two options:

1. Push the branch and watch CI (5 min feedback loop).
2. Run inside the Linux container shipped at `docker-compose.e2e.yml`:

   ```bash
   pnpm e2e:docker
   # equivalent to: docker compose -f docker-compose.e2e.yml run --build --rm e2e
   ```

   Source is COPY'd into the image at build time — no bind mount of the working tree — so the run matches CI. `--build` re-COPYs after edits. Named volumes persist node_modules, the cargo target dir, and the pnpm/cargo download caches across runs, so incremental builds stay fast. Failure artifacts surface in `./e2e-results/`.

## One-time local setup (Linux)

```bash
# System packages (Ubuntu 24.04 names; adjust per distro)
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libgtk-3-dev \
  webkit2gtk-driver \
  xvfb \
  libasound2-dev

# Silent ALSA device (rodio needs an output device even with WAV silence)
sudo modprobe snd-dummy

# tauri-driver itself
cargo install tauri-driver --version 2.0.6 --locked

# JS deps
pnpm install
```

## Run

```bash
# Debug build (default, faster recompile)
cargo build --manifest-path src-tauri/Cargo.toml
pnpm vite build
pnpm e2e

# Release build (matches CI)
pnpm tauri build --no-bundle
E2E_BINARY=release pnpm e2e
```

The renderer must be prebuilt (`pnpm vite build`) before the Rust binary will boot — `tauri::generate_context!` validates `frontendDist=../dist` at compile time.

Headless: prefix with `xvfb-run -a` if you don't have a display server.

## Architecture

- **Per-test launch.** Each `it()` spawns a fresh app process with its own `XDG_DATA_HOME=/tmp/radiodiodj-e2e-XXXX`, so `config.json`, `radiodiodj.db`, and `session.json` are hermetic.
- **Fixture libraries** are synthesized at runtime as 1-second 16-bit mono PCM WAVs. No binaries committed.
- **Selectors** prefer ARIA (`role`, `aria-label`, `aria-sort`) over CSS classes; falls back to stable `id="..."` attributes.
- **Audio** in CI uses `snd-dummy`; rodio decodes through symphonia and writes to the dummy device, so `main-deck:time` advances normally.

## Failure artifacts

On test failure, the runner writes to `e2e-results/<spec-name>/`:

- `failure.png` — screenshot
- `tauri-driver.log` — driver stderr/stdout
- per-test temp dir snapshot (logs, session.json, radiodiodj.db) when copied by the test's `afterEach`

CI uploads this directory as an artifact with 7-day retention.

## Adding a spec

```ts
import { launchApp, captureArtifacts } from "../launch";
import { sel } from "../selectors";

describe("my feature", () => {
  let cleanup: () => Promise<void>;

  afterEach(async function () {
    if (this.currentTest?.state === "failed") {
      await captureArtifacts(this.currentTest.fullTitle());
    }
    await cleanup();
  });

  it("does the thing", async () => {
    const app = await launchApp({
      musicPaths: [/* fixture dir */],
    });
    cleanup = app.cleanup;
    // ... assertions
  });
});
```
