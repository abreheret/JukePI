#!/usr/bin/env bash
# Build the files of a GitHub release into dist/ (git-ignored): the server binary for the Raspberry Pi and the APK.
#
#   ./scripts/build-release.sh
#
# Needs what the project needs (zig, cargo-zigbuild, a JDK 21, the Android SDK; see the READMEs). The binary is built
# with the paths of your machine remapped, and both files are scanned for what must never be published: your user
# name, your home folder, your git e-mail and any secret you have exported (LASTFM_*). The script stops if it finds one.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$(pwd)"
export JAVA_HOME="${JAVA_HOME:-/opt/homebrew/opt/openjdk@21}"
mkdir -p dist

echo "== server (aarch64)"
( cd music-server
  RUSTFLAGS="--remap-path-prefix=$HOME=/build/home --remap-path-prefix=$ROOT=/build/src" \
    cargo zigbuild --release --target aarch64-unknown-linux-gnu.2.36 )
cp music-server/target/aarch64-unknown-linux-gnu/release/music-server dist/music-server-linux-aarch64

echo "== Android APK"
( cd android-app && ./gradlew assembleRelease -q )
VERSION="$(sed -n 's/.*versionName = "\(.*\)".*/\1/p' android-app/app/build.gradle.kts | head -1)"
cp android-app/app/build/outputs/apk/release/app-release.apk "dist/JukePI-v${VERSION}.apk"

echo "== scan for personal data and secrets"
patterns=("$HOME" "$(whoami)" "/Users/")
email="$(git config user.email || true)"; [ -n "$email" ] && patterns+=("$email")
for v in LASTFM_API_KEY LASTFM_SECRET; do [ -n "${!v:-}" ] && patterns+=("${!v}"); done
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
strings -a dist/music-server-linux-aarch64 > "$tmp/server.txt"
( cd "$tmp" && mkdir apk && cd apk && unzip -q -o "$ROOT/dist/JukePI-v${VERSION}.apk" )
find "$tmp/apk" -type f \( -name '*.dex' -o -name '*.xml' -o -name '*.arsc' -o -name '*.properties' \) -exec strings -a {} + > "$tmp/apk.txt" 2>/dev/null || true
bad=0
for p in "${patterns[@]}"; do
  [ -z "$p" ] && continue
  for f in server apk; do
    if grep -q -F -- "$p" "$tmp/$f.txt"; then echo "FOUND in $f: '$p'" >&2; bad=1; fi
  done
done
[ "$bad" = 0 ] || { echo "Personal data found: not publishing." >&2; exit 1; }
echo "clean"

( cd dist && shasum -a 256 music-server-linux-aarch64 "JukePI-v${VERSION}.apk" > SHA256SUMS && cat SHA256SUMS )
