#!/usr/bin/env bash
# Build for the Raspberry Pi (aarch64) on your computer, upload the binary and restart the systemd service.
#
#   PI=pi@raspberrypi.local ./deploy/deploy.sh
#
# Needs: zig (brew install zig), cargo-zigbuild (cargo install cargo-zigbuild --locked),
#        rustup target add aarch64-unknown-linux-gnu. `sudo` on the Pi asks for its password (ssh -t).
#
# The systemd unit (deploy/music-server.service) is installed ONLY if the Pi does not have one yet, because it
# holds YOUR settings (music folder, MUSIC_PUBLIC_URL...). Edit the copy on the Pi afterwards. To overwrite an
# existing unit on purpose: FORCE_UNIT=1 PI=... ./deploy/deploy.sh
set -euo pipefail
PI="${PI:?set PI=user@host (the SSH target of the Raspberry Pi)}"
cd "$(dirname "$0")/.."

cargo test
cargo zigbuild --release --target aarch64-unknown-linux-gnu.2.36   # .2.36 = glibc floor (Raspberry Pi OS has >= 2.36)
BIN=target/aarch64-unknown-linux-gnu/release/music-server

ssh "$PI" 'mkdir -p ~/music-server'
scp "$BIN" "$PI:music-server/music-server.new"
scp deploy/music-server.service "$PI:/tmp/music-server.service"
scp deploy/remote-install.sh "$PI:/tmp/jukepi-install.sh"
# A real terminal (ssh -t) is needed for the sudo password prompt; that is why the remote steps are a file, not stdin.
ssh -t "$PI" "FORCE_UNIT=${FORCE_UNIT:-0} bash /tmp/jukepi-install.sh; rm -f /tmp/jukepi-install.sh"
