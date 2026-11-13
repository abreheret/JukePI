# JukePI

🇫🇷 [Français](README.fr.md)

[![Server](https://github.com/abreheret/JukePI/actions/workflows/server.yml/badge.svg)](https://github.com/abreheret/JukePI/actions/workflows/server.yml) [![Android](https://github.com/abreheret/JukePI/actions/workflows/android.yml/badge.svg)](https://github.com/abreheret/JukePI/actions/workflows/android.yml) [![Release](https://img.shields.io/github/v/release/abreheret/JukePI)](https://github.com/abreheret/JukePI/releases/latest) [![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

**Self-hosted music server for the Raspberry Pi, with an Android remote that drives a Denon HEOS amplifier or plays offline on the phone.**

Your MP3 library lives on a Raspberry Pi. The Pi indexes it and serves it over HTTP; a **Denon HEOS** amplifier downloads the tracks itself and plays them, while an **Android app** shows your albums (covers, search, artists) and controls the amplifier. Away from home the same app plays **downloaded albums on the phone**.

```
 Android app ──HTTP/JSON──▶  Raspberry Pi: JukePI server (Rust)  ──HEOS CLI──▶  Denon HEOS amplifier
   │  (remote, and a player                │  SQLite index + MP3 files                  │
   │   of its own when offline)            └────────── GET /stream/{id}.mp3 ◀───────────┘
   └─ downloaded albums on the phone
```

## Preview

<table><tr><td align="center"><img src="docs/screenshots/albums.png?v=4" width="190" alt="Albums"><br><sub>Albums</sub></td><td align="center"><img src="docs/screenshots/album.png?v=2" width="190" alt="Album"><br><sub>Album</sub></td><td align="center"><img src="docs/screenshots/search.png?v=2" width="190" alt="Search"><br><sub>Search</sub></td><td align="center"><img src="docs/screenshots/player.png?v=2" width="190" alt="Now playing"><br><sub>Now playing</sub></td><td align="center"><img src="docs/screenshots/downloaded.png?v=2" width="190" alt="Downloaded"><br><sub>Downloaded</sub></td></tr></table>

Screenshots of the Android app (the interface is currently in French). Album covers belong to their respective owners and are shown for illustration only.

## What is in this repository

| Folder | What | Language |
| --- | --- | --- |
| [`music-server/`](music-server/README.md) | The server: scan + index, API, streaming, covers, full-text search, HEOS client, player, mDNS | Rust (`axum`, `tokio`, SQLite) |
| [`android-app/`](android-app/README.md) | The Android app: browse, control the amplifier, play on the phone, download albums, offline mode | Kotlin, Jetpack Compose, Media3 |

Each folder has its own README, an `ARCHITECTURE` document and development notes. Documentation is in **English by default** and also available in **French** (`*.fr.md`).

## Highlights

- **No audio through the phone** when playing on the amplifier: the amplifier fetches the MP3 files from the Pi. The phone is a remote.
- Indexes tens of thousands of MP3 files in minutes, rescans in seconds; handles messy tags, compilations, missing artwork.
- Covers from the album folder or the files' embedded pictures, thumbnails cached; optional online lookup (off by default).
- Accent-insensitive full-text search answering in about 15 ms, tolerant to typos ("beatels" finds The Beatles), one entry per artist whatever the spelling.
- The server announces itself with mDNS: the app finds it without typing an address.
- Seek, mute, volume and queue control for the amplifier (HEOS has no seek command: the server restarts the stream at the wanted frame).
- **"More like this"**: when the queue reaches its last track, the app keeps going with tracks of your library that are similar (Last.fm similarity when configured, your genres otherwise), leaning towards the artists you listen to most. What the amplifier plays can be **scrobbled** to your Last.fm account.
- Android: lock-screen and notification controls, volume keys drive the amplifier, **downloads** with resume, **offline mode** with local playback, switchable output (amplifier / phone) that carries the queue over.
- Developed against **Android 17**, including its new local-network permission.

## Requirements

- A Raspberry Pi (tested on a Pi 4, 4 GB, Raspberry Pi OS / Debian 13) with the MP3 library on a disk.
- A Denon (or other) amplifier speaking the **HEOS CLI** protocol on the same network. Developed and tested on a **Denon DRA-900H**; other HEOS devices are untested.
- An Android phone, Android 14 or newer to build as is (`minSdk 34`, developed on Android 17).
- To build: Rust (+ `zig` and `cargo-zigbuild` to cross-compile for the Pi), a JDK 21 and the Android SDK.

## Quick start

1. **Server**: see [`music-server/README.md`](music-server/README.md) and [`music-server/docs/OPERATIONS.md`](music-server/docs/OPERATIONS.md).
   ```bash
   cd music-server
   cp deploy/music-server.env.example deploy/music-server.env   # git ignores this file: your settings and secrets go here
   $EDITOR deploy/music-server.env                              # MUSIC_PUBLIC_URL (the Pi's address as the amplifier sees it), MUSIC_ROOT, Last.fm...
   PI=pi@raspberrypi.local ./deploy/deploy.sh                   # tests, build, upload, install, restart
   ```
   **Optional, Last.fm** ("more like this" and scrobbling): create an application on <https://www.last.fm/api/account/create>, put its key and shared secret in `deploy/music-server.env` (`LASTFM_API_KEY`, `LASTFM_SECRET`), redeploy, then link your account once: `MUSIC_SERVER=<pi-ip>:8765 ./scripts/lastfm-link.sh`.
2. **App**: see [`android-app/README.md`](android-app/README.md): `./android-app/install.sh` with a phone connected over USB.
3. Open the app: it finds the server by itself (or type its address in Settings), grant the local-network permission, and play.

## Status

Working and used day to day on the author's setup (a 43,794-track library, a Pi 4 and a DRA-900H). Some paths have not been exercised end to end; each project's roadmap lists them. Contributions and reports from other setups are welcome.

## Privacy and security

- The API has **no authentication**: run it on a trusted local network only.
- The optional online cover lookup (`COVERS_ONLINE=1`) sends artist and album names to MusicBrainz and the Cover Art Archive. It is off by default.
- The optional Last.fm features (`LASTFM_API_KEY`, `LASTFM_SECRET`) send the artist and title being played to Last.fm, and with scrobbling the list of what the amplifier plays. They are off without a key; secrets live in the systemd unit of the Pi, never in the repository. See [OPERATIONS](music-server/docs/OPERATIONS.md#enabling-lastfm-privacy).
- IP addresses in the documentation (`192.168.1.50`, `192.168.1.60`) are examples. Nothing in the repository holds credentials.

## Licence and trademarks

Released under the [MIT licence](LICENSE). *Denon* and *HEOS* are trademarks of their respective owners; this project is independent and not affiliated with or endorsed by them.
