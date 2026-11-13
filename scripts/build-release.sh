#!/usr/bin/env bash
# Build the files of a GitHub release into dist/ (git-ignored): the server binary for the Raspberry Pi and the APK.
#
#   ./scripts/build-release.sh
#
# Needs what the project needs (zig, cargo-zigbuild, a JDK 21, the Android SDK; see the READMEs). The binary is built
# with the paths of your machine remapped, and both files are scanned for what must never be published: your user
# name, your home folder, your git e-mail, the signing password and any secret you have exported (LASTFM_*). The
# script stops if it finds one.
#
# The APK is signed with YOUR release key: ~/.jukepi/release.jks (JUKEPI_KEYSTORE to use another file), alias
# `jukepi`, password from JUKEPI_KEYSTORE_PASSWORD or, on macOS, the Keychain item `jukepi-release-keystore`. Create
# them once (see music-server/docs/OPERATIONS.md). Without the key the script stops: a release signed with the debug
# key cannot be updated later. ALLOW_DEBUG_SIGNING=1 builds one anyway (to try the script).
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
KEYSTORE="${JUKEPI_KEYSTORE:-$HOME/.jukepi/release.jks}"
if [ -f "$KEYSTORE" ]; then
  if [ -z "${JUKEPI_KEYSTORE_PASSWORD:-}" ] && command -v security >/dev/null; then
    JUKEPI_KEYSTORE_PASSWORD="$(security find-generic-password -s jukepi-release-keystore -w 2>/dev/null || true)"
  fi
  [ -n "${JUKEPI_KEYSTORE_PASSWORD:-}" ] || { echo "No password for $KEYSTORE (JUKEPI_KEYSTORE_PASSWORD or the Keychain item jukepi-release-keystore)." >&2; exit 1; }
  export JUKEPI_KEYSTORE="$KEYSTORE" JUKEPI_KEYSTORE_PASSWORD
  SIGNED=1
elif [ "${ALLOW_DEBUG_SIGNING:-0}" = 1 ]; then
  echo "WARNING: no release key, the APK is signed with the debug key" >&2
  SIGNED=0
else
  echo "No release key at $KEYSTORE: refusing to build a release that could never be updated." >&2
  exit 1
fi
( cd android-app && ./gradlew assembleRelease -q )
VERSION="$(sed -n 's/.*versionName = "\(.*\)".*/\1/p' android-app/app/build.gradle.kts | head -1)"
cp android-app/app/build/outputs/apk/release/app-release.apk "dist/JukePI-v${VERSION}.apk"

echo "== scan for personal data and secrets"
names=("home folder" "user name" "/Users/"); patterns=("$HOME" "$(whoami)" "/Users/")
email="$(git config user.email || true)"; [ -n "$email" ] && { names+=("git e-mail"); patterns+=("$email"); }
for v in LASTFM_API_KEY LASTFM_SECRET JUKEPI_KEYSTORE_PASSWORD; do
  [ -n "${!v:-}" ] && { names+=("$v"); patterns+=("${!v}"); }
done
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
strings -a dist/music-server-linux-aarch64 > "$tmp/server.txt"
( cd "$tmp" && mkdir apk && cd apk && unzip -q -o "$ROOT/dist/JukePI-v${VERSION}.apk" )
find "$tmp/apk" -type f \( -name '*.dex' -o -name '*.xml' -o -name '*.arsc' -o -name '*.properties' \) -exec strings -a {} + > "$tmp/apk.txt" 2>/dev/null || true
bad=0
for i in "${!patterns[@]}"; do
  p="${patterns[$i]}"; [ -z "$p" ] && continue
  for f in server apk; do
    # The name of what was found is printed, never its value (it can be a secret).
    if grep -q -F -- "$p" "$tmp/$f.txt"; then echo "FOUND in $f: ${names[$i]}" >&2; bad=1; fi
  done
done
[ "$bad" = 0 ] || { echo "Personal data found: not publishing." >&2; exit 1; }
echo "clean"

echo "== APK signature"
APKSIGNER="${ANDROID_HOME:-/opt/homebrew/share/android-commandlinetools}/build-tools/$(ls "${ANDROID_HOME:-/opt/homebrew/share/android-commandlinetools}/build-tools" | sort -V | tail -1)/apksigner"
cert="$("$APKSIGNER" verify --print-certs "dist/JukePI-v${VERSION}.apk")"
echo "$cert" | sed -n '1,3p'
if [ "${SIGNED:-0}" = 1 ] && echo "$cert" | grep -q "Android Debug"; then echo "The APK is signed with the DEBUG key although a release key exists." >&2; exit 1; fi

( cd dist && shasum -a 256 music-server-linux-aarch64 "JukePI-v${VERSION}.apk" > SHA256SUMS && cat SHA256SUMS )
