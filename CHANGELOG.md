# Changelog

🇫🇷 Le journal des modifications est en anglais ; la documentation existe en français (`*.fr.md`).

## Unreleased

- Suggestions come right after an album starts: 10 close tracks are appended to the queue, marked with a star and a "Suggestions" heading, so you can see and skip them; 10 more when the last track starts.
- A "All albums of <artist>" button on the album page and on the full-screen player opens the artist's page.
- The "Recent" tab shows the albums you listened to first (listening history kept by the server: `GET /albums?sort=listened`, `POST /history/played`, `DELETE /history`), then albums with a cover.

## 0.1.0 - 2026-11-13

First public release.

### Server (`music-server`, Rust, Raspberry Pi)
- Scans an MP3 library into a SQLite index (43,794 tracks in about 2.5 minutes, rescans in seconds); albums = folders.
- HTTP API: albums, artists, tracks, accent-insensitive full-text search (about 15 ms) that tolerates typos, one entry per artist whatever the spelling, covers (folder image, embedded picture, cached thumbnails, optional online lookup, off by default), streaming with `Range`.
- The amplifier shows the title, artist and album of what it plays (an ID3 tag built from the index, exact lengths read from the VBR headers).
- HEOS client (SSDP discovery, events, reconnection) and a player that owns the queue: automatic chaining, pause, next/previous, volume, mute, seek (the stream is restarted at the wanted MP3 frame).
- mDNS announcement (`_music-server._tcp`) so the app finds the server.
- **Last.fm** (optional, off without a key): "more like this" suggestions matched against the library and lifted by the account's listening history; scrobbling of what the amplifier plays; account linked in the browser.
- systemd service, one-command deployment (`deploy/deploy.sh`, also with a prebuilt binary), settings and secrets in a git-ignored file installed root-only on the Pi.

### Android app (`android-app`, Kotlin / Jetpack Compose, Android 14+)
- Server discovery, album grid, artists, search, mini and full-screen player (draggable timeline, +/-10 s and +/-30 s, volume, mute, queue).
- Lock-screen and notification controls; the phone's volume keys drive the amplifier.
- Output choice amplifier / phone with queue and position carried over.
- Album downloads with resume, "Downloaded" tab, **offline mode** with local playback.
- "Same style" button and automatic continuation when the queue ends.

### Known limits
- No authentication on the API: use a trusted local network.
- Developed and tested on a Raspberry Pi 4 and a Denon DRA-900H; other HEOS devices are untested.
- The APK is signed with a debug key (see `android-app/README.md`).
- Not yet exercised end to end: output hand-over Denon <-> phone while playing, an interrupted download resumed after killing the app, a reboot of the Pi with the SSD mount, a full `deploy.sh` run.
