#!/usr/bin/env bash
# Build for the Raspberry Pi (aarch64) on your computer, upload the binary and restart the systemd service.
#
#   PI=pi@raspberrypi.local ./deploy/deploy.sh
#
# Needs: zig (brew install zig), cargo-zigbuild (cargo install cargo-zigbuild --locked),
#        rustup target add aarch64-unknown-linux-gnu. `sudo` on the Pi asks for its password (ssh -t).
#
# No Rust toolchain? Use a binary from the GitHub release: PREBUILT=/path/to/music-server-linux-aarch64 PI=... ./deploy/deploy.sh
# (skips the tests and the build).
#
# Settings and secrets: copy deploy/music-server.env.example to deploy/music-server.env (git ignores it) and fill it in.
# It is uploaded to /etc/music-server/music-server.env on the Pi (root only, mode 600). Without that file the Pi
# keeps the settings it already has.
#
# The systemd unit (deploy/music-server.service) is installed ONLY if the Pi does not have one yet, because it
# holds YOUR settings (music folder, MUSIC_PUBLIC_URL...). Edit the copy on the Pi afterwards. To overwrite an
# existing unit on purpose: FORCE_UNIT=1 PI=... ./deploy/deploy.sh
set -euo pipefail
PI="${PI:?set PI=user@host (the SSH target of the Raspberry Pi)}"
cd "$(dirname "$0")/.."

if [ -n "${PREBUILT:-}" ]; then
  BIN="$PREBUILT"
  [ -f "$BIN" ] || { echo "PREBUILT=$BIN not found" >&2; exit 1; }
else
  cargo test
  cargo zigbuild --release --target aarch64-unknown-linux-gnu.2.36   # .2.36 = glibc floor (Raspberry Pi OS has >= 2.36)
  BIN=target/aarch64-unknown-linux-gnu/release/music-server
fi

ssh "$PI" 'mkdir -p ~/music-server'
scp "$BIN" "$PI:music-server/music-server.new"
scp deploy/music-server.service "$PI:/tmp/music-server.service"
scp deploy/music-server-envfile.conf "$PI:/tmp/music-server-envfile.conf"
ENVFILE="${ENVFILE:-deploy/music-server.env}"
if [ -f "$ENVFILE" ]; then
  echo "settings: $ENVFILE -> /etc/music-server/music-server.env on the Pi (root only)"
  if grep -Eq '^MUSIC_PUBLIC_URL=.*192\.168\.1\.50' "$ENVFILE"; then
    echo "WARNING: MUSIC_PUBLIC_URL is still the example address 192.168.1.50: is that really your Pi?" >&2
  fi
  # Through stdin with a umask: the secrets are never world-readable, not even in /tmp.
  ssh "$PI" 'umask 077; cat > /tmp/music-server.env' < "$ENVFILE"
else
  echo "no $ENVFILE: the Pi keeps its current settings (copy deploy/music-server.env.example to create one)"
fi
scp deploy/remote-install.sh "$PI:/tmp/jukepi-install.sh"
# A real terminal (ssh -t) is needed for the sudo password prompt; that is why the remote steps are a file, not stdin.
ssh -t "$PI" "FORCE_UNIT=${FORCE_UNIT:-0} bash /tmp/jukepi-install.sh; rm -f /tmp/jukepi-install.sh"
