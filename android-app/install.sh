#!/usr/bin/env bash
# Build the debug APK and install it on the USB-connected phone (USB debugging on, computer authorised).
# The defaults below are Homebrew paths on macOS: set JAVA_HOME / ANDROID_HOME for your machine.
set -euo pipefail
cd "$(dirname "$0")"
export JAVA_HOME="${JAVA_HOME:-/opt/homebrew/opt/openjdk@21}"
export ANDROID_HOME="${ANDROID_HOME:-/opt/homebrew/share/android-commandlinetools}"
ADB="$ANDROID_HOME/platform-tools/adb"
./gradlew assembleDebug
"$ADB" devices -l
"$ADB" install -r app/build/outputs/apk/debug/app-debug.apk
# After `install -r` the old process is dead but Android may still "bring its task to the front": stop it first.
"$ADB" shell am force-stop app.musicremote
"$ADB" shell am start -n app.musicremote/.MainActivity
