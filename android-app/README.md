# JukePI Android app

🇫🇷 [Français](README.fr.md)

The Android half of JukePI. It browses your library (covers, albums, artists, search) and either **drives a Denon HEOS amplifier** through the JukePI server, or **plays on the phone itself**, including **offline** with albums downloaded to the phone. The server is in [`../music-server`](../music-server/README.md).

Target: **Android 17 (API 37)** (developed on a Pixel 8a). Kotlin, Jetpack Compose, Material 3 (dynamic colours). The package id is `app.musicremote`, the launcher name is "JukePI".

## Features

| Feature | Status |
| --- | --- |
| Local-network permission flow (required on Android 17) | done, tested |
| Automatic server discovery (mDNS) and manual address entry | done, tested |
| Album grid with covers, sorting (recent, A–Z, artist, random), infinite scroll | done, tested |
| Album page, artists, full-text search (type-ahead) | done, tested |
| Mini player and full-screen player (previous, pause, next, stop, queue) | done, tested |
| Draggable timeline, ±10 s and ±30 s buttons | done, tested (on the amplifier the server restarts the stream: ~1 s of silence; on the phone seeking is instant) |
| Volume (phone volume keys drive the amplifier), mute | done, tested |
| Notification and lock-screen controls (Media3 `MediaSession`, five buttons incl. ±10 s) | done; notification buttons tested; lock screen look not checked visually |
| Hardware media keys (Bluetooth headset buttons) | work when the output is the phone (see Known limits) |
| Adaptive app icon (with a monochrome layer for themed icons) | done |
| **Output choice Amplifier / Phone** in the full-screen player (queue and position carry over) | done; selector verified, hand-over not tested yet |
| **Play on the phone** (ExoPlayer): downloaded files, or streamed from the server | done, tested |
| **Download albums** (Download / Cancel / Remove on the album page, resumes after interruption) | done, tested (6 tracks, 47 MB in ~3 s) |
| **"Downloaded" tab**: everything on the phone, state and size of each album, free space, management | done, verified |
| **Offline mode**: automatic detection, banner, albums / artists / search limited to downloaded music, output forced to the phone | done, tested by cutting the network |
| Interrupted download resumes at the first missing track | written; not tested by killing the app mid-download |

## Known limits

- **Hardware media keys** (Bluetooth headset buttons) only reach the app when the output is the **phone**: Android sends them to the app that actually plays audio locally. With the amplifier as output, the notification and lock-screen buttons work (they talk to the session directly) but the headset buttons do not. Working around it would need a silent audio track playing on the phone: not done.
- Offline, a track that was not downloaded cannot be played (dimmed on the album page).
- On the amplifier, seeking inside a track costs **~1 s of silence** (the server restarts the stream at the wanted spot).
- The ±30 s buttons exist only in the full-screen player; the notification and lock screen have ±10 s (Android allows five buttons at most).

## Build and install

Prerequisites: a JDK 21, the Android SDK command-line tools with platform 37, build-tools 37 and platform-tools, and Gradle only to create the wrapper (already in the repository). On macOS with Homebrew: `brew install openjdk@21` and `brew install --cask android-commandlinetools`, then `sdkmanager "platform-tools" "platforms;android-37.0" "build-tools;37.0.0"` and accept the licences (`sdkmanager --licenses`).

```bash
echo "sdk.dir=/path/to/android/sdk" > local.properties     # ignored by git
./install.sh        # builds, installs on the USB-connected phone, launches the app
```

Or by hand:

```bash
export JAVA_HOME=/path/to/jdk-21 ANDROID_HOME=/path/to/android/sdk
./gradlew assembleDebug
$ANDROID_HOME/platform-tools/adb install -r app/build/outputs/apk/debug/app-debug.apk
```

`JAVA_HOME` must point at **JDK 21**: Homebrew's `gradle` starts on a newer JDK than the Android plugin accepts. The phone needs **USB debugging** enabled and the computer authorised (`adb devices` must show `device`).

## Versions (as of November 2026)

Android Gradle Plugin 9.5.1 (built-in Kotlin), Kotlin 2.4.21 (Compose and serialization plugins), Gradle 9.9.1, Compose BOM 2026.09.00, OkHttp 5.5.1 (+ okhttp-sse), kotlinx.serialization 1.13.0, Coil 3.6.3, Media3 1.11.1 (session + ExoPlayer), WorkManager 2.12.0, `compileSdk` 37, `targetSdk` 37, `minSdk` 34 (gives `NsdManager.registerServiceInfoCallback`).

## Documentation

| File | Content |
| --- | --- |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Code layout, data flow, decisions and pitfalls |
| [docs/NOTES.md](docs/NOTES.md) | Development notes: lessons learned, testing tips, roadmap |
| [`../music-server/docs/ANDROID-CLIENT.md`](../music-server/docs/ANDROID-CLIENT.md) | The server side of the contract: API, mDNS, live stream |
