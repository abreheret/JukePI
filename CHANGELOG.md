# Changelog

🇫🇷 Le journal des modifications est en anglais ; la documentation existe en français (`*.fr.md`).

## Unreleased

### Added
- **Albums downloaded by a torrent client join the library by themselves**, filed as `<Artist>/<Album>/` from their tags like the rest of the library (`Various Artists` for compilations, CD1/CD2 kept, several albums per torrent). `music-server/deploy/torrent-albums/` copies each finished qBittorrent download that contains MP3s (MP3 files and covers only), never overwrites an album already there, imports each torrent once (deleting the torrent keeps the music) and starts a rescan. See docs/OPERATIONS.md.
- The scanner follows symbolic links (albums stored elsewhere can be linked into the library).

## 0.3.0 - 2026-11-14

### Added
- **Endless queue, kept by the server.** With the amplifier, the server itself appends 10 suggestions when a queue starts and 10 more when the last or last-but-one track plays, so the music goes on even with no app open. Tracks played recently (300) and tracks already queued are never suggested again; a queue that grows to 1000 forgets what was played long ago. Suggested tracks carry a `reason` in the player snapshot, so every phone marks them the same way (new fields `reason` and `auto_suggest`).
- **Endless scrolling in the app**: scroll the queue down and 10 more suggestions are added each time you get near its end.
- **Download the queue as a playlist.** A "Download" button next to the queue in the player keeps the whole queue on the phone under an automatic name you can edit (main artist + date, e.g. "Michael Jackson and others · Nov 14"). Playlists appear in the "Downloaded" tab, play without the server, can be renamed, resumed and deleted; tracks already on the phone are copied instead of downloaded again. No server change.
- **Swipe a track of the queue left or right to take it out** (the track being played cannot be removed). A removed suggestion is not proposed again soon. `POST /player/remove`.
- `POST /player/suggest_more` and `POST /player/auto_suggest`. The "Suggestions after the queue" setting now lives on the server (shared by all phones, kept across restarts).
- **A new icon**: a gold J-shaped music note (J for jukebox) in front of a raspberry (the Raspberry Pi) on a black background; the themed icon of Android 13+ keeps the J.
- **The app speaks your language**: English, French, German, Italian, Spanish, Portuguese (European, with a Brazilian variant), Dutch, Catalan, Swedish, Danish, Norwegian, Finnish and Polish, picked from the phone's language (Android 13+ also lists them in Settings > Apps > JukePI > Language). Every text, notification and accessibility label is in `res/values*/strings.xml` (the default is English); sizes, dates and plurals follow the language. The automatic name of a playlist is built in the language of the moment ("Michael Jackson and others · Nov 14"); playlists saved before keep their name.

### Fixed
- **A song is no longer suggested twice in two versions.** A suggestion is skipped when the same artist's song is already queued (or was played recently) under another title spelling: "Song (Live)", "Song - 2011 Remaster", "Song (Remastered)"... count as one song. Within one batch, only one version of a song is kept.

### Upgrading
- Update the server and the app together (0.3.0 both). An app of 0.2.0 with the new server would not know the suggestions the server adds and would add its own batch too.

## 0.2.0 - 2026-11-13

### Upgrading
- **The APK is now signed with the project's release key** (certificate SHA-256 in `android-app/README.md`). v0.1.0 was signed with a debug key, and Android refuses to install an app over one signed with another key: **uninstall v0.1.0 once** (this deletes the downloaded albums, which are downloaded again in a few seconds). Every later release updates in place.
- The server needs no migration: the index gets its new columns and tables by itself, and the durations of the existing tracks are repaired once in the background (about 2.5 minutes for 40,000 tracks).

### Added
- **Suggestions behind what you play.** As soon as an album starts, 10 tracks of your library that are close to it are appended to the queue, marked with a star, the reason (Last.fm, same genre, same artist) and a "Suggestions" heading, so you can see and skip them; 10 more when the last track starts. A setting turns it off.
- **"Recent" shows what you listened to** (history kept by the server: `GET /albums?sort=listened`, `POST /history/played`, `DELETE /history`), then albums with a cover.
- **"All albums of this artist" button** on the album page and in the full-screen player.
- **Typo-tolerant search** ("beatels" finds The Beatles, "acdc" finds AC/DC) and **one entry per artist** whatever the spelling ("Queen"/"QUEEN"). "Close matches" are labelled in the app.
- `scripts/build-release.sh`: builds the release files, signs with the release key, refuses a debug-signed APK and checks that no personal data or secret is in the files.

### Fixed
- **The app crashed when the queue held the same track twice** ("Duplicate MediaItemData UID"): two clients asking for suggestions at the same moment appended the same ten tracks twice. The player now gives every queue entry a unique id, and the server no longer adds a track that is already queued.
- **The app crashed at its first launch on a fresh install** (no saved server address), v0.1.0 included: a signal used by the reachability probe was declared after the `init` block that starts the probe, so it was still null. It never showed on a phone that already knew the server. Found by installing the APK on a phone that had never had the app.
- **The Denon shows the title and artist of every track**, not only the variable-bitrate ones. The Denon only reads the ID3 tag of a stream when it sees the MPEG frame headers change, which a constant-bitrate file never does: such files now get one silent 26 ms frame with a differing header half a second in. Titles without tags show their name without the track number and the artist is appended to the title line (the front panel only scrolls the title). Accented titles are written as UTF-16. Measured on the real Denon: every one of 22 files tested now shows its title.
- Exact track lengths are read from the VBR header (the index announced 36 minutes for a 4 minute track).

### Known limits
- Developed and tested on a Raspberry Pi 4 and a Denon DRA-900H only. No authentication on the API (trusted local network). The amplifier shows the title 3 to 5 seconds after a track starts, and not the cover.
- Not yet exercised end to end: output hand-over Denon <-> phone while playing, resuming a download interrupted by killing the app, a reboot of the Pi, a full `deploy.sh` run on a fresh Pi, installing the APK on a phone that never had the app.

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
