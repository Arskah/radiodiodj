#!/usr/bin/env bash
# Entrypoint for Dockerfile.e2e. Starts PulseAudio null-sink, ensures
# dependencies + release binary exist, then runs `pnpm e2e` under xvfb.
set -euxo pipefail

# Hermetic per-container Pulse state (avoids host socket reuse).
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/tmp/xdg}"
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"

pulseaudio --start --exit-idle-time=-1 --log-target=stderr
pactl load-module module-null-sink sink_name=dummy sink_properties=device.description=Dummy
pactl set-default-sink dummy

export E2E_BINARY="${E2E_BINARY:-release}"

# Build release binary if missing. node_modules + cargo deps are baked into
# the image; the target dir lives in a named volume for incremental rebuilds.
if [ ! -f "src-tauri/target/${E2E_BINARY}/radiodiodj" ]; then
  pnpm tauri build --no-bundle
fi

mkdir -p e2e-results

# Start Xvfb directly instead of via xvfb-run. xvfb-run waits for SIGUSR1
# from Xvfb to signal readiness; inside this container that handshake
# blocks indefinitely and the wrapped command never starts.
Xvfb :99 -screen 0 1280x1024x24 -nolisten tcp >e2e-results/xvfb.log 2>&1 &
export DISPLAY=:99
# Wait for Xvfb's UNIX socket to appear (readiness signal without xset).
for _ in $(seq 1 50); do
  if [ -S /tmp/.X11-unix/X99 ]; then break; fi
  sleep 0.1
done

exec pnpm e2e
