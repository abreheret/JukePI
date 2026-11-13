# Architecture

🇫🇷 [Français](ARCHITECTURE.fr.md)

## Overview

A single Rust binary (`axum` + `tokio`), started by systemd on the Raspberry Pi. It does four things:

1. **Index** the MP3 library into SQLite (`scan.rs`, `db.rs`).
2. **Serve** the index as JSON and the MP3 files over HTTP with `Range` requests (`api.rs`).
3. **Talk to the amplifier** with HEOS CLI over TCP (`heos.rs`).
4. **Drive playback**: queue, chaining of tracks, real-time state (`player.rs`).

Playback is driven from the Pi, not from the phone, so it keeps going when the phone goes to sleep.

```
 app / curl ──▶ api.rs ──▶ player.rs (actor) ──▶ heos.rs ──TCP 1255──▶ amplifier
                  │              ▲                    │
                  │              └── events ◀─────────┘ (state_changed, progress, error...)
                  ▼
               SQLite ◀── scan.rs ◀── music library (disk, read-only)

 amplifier ──GET /stream/{id}.mp3──▶ api.rs ──ServeFile (Range)──▶ MP3 file
```

## Modules

| File | Role | Things to know |
| --- | --- | --- |
| `main.rs` | Configuration from environment variables; starts scan, HEOS client, player, HTTP server | The API answers immediately, the scan runs in the background |
| `db.rs` | Opens SQLite (WAL, `synchronous=NORMAL`), creates the schema | `open` for the scan, a light `connect` for each API request |
| `mp3seek.rs` | Finds the byte where the MP3 frame of a given time starts | Walks the frames one by one (CBR and VBR) |
| `mdns.rs` | Announces the server on the local network (mDNS / DNS-SD) | Single interface, the one of `MUSIC_PUBLIC_URL` |
| `search.rs` | Full-text index (SQLite FTS5) | Rebuilt after a scan that changed something |
| `covers.rs` | Finds each album's cover source, builds and caches thumbnails | See the "Covers" section |
| `online.rs` | Optional online cover lookup | Off by default |
| `scan.rs` | Walks the disk, reads tags (`lofty`), updates the index | Incremental on (path, size, mtime); see below |
| `api.rs` | HTTP routes, JSON errors, SSE stream | SQLite access goes through `spawn_blocking` |
| `heos.rs` | TCP connection to the amplifier, commands, events, SSDP | One task owns the socket; automatic reconnection |
| `player.rs` | Single actor owning the queue and the playback state | Avoids any race between commands and events |

## SQLite index

Two tables (`db.rs`).

- `albums(id, dir UNIQUE, dir_name, name, artist, artist_count, year, track_count, added_at)`
- `tracks(id, path UNIQUE, album_id, title, artist, album_tag, genre, track_no, disc_no, year, duration_ms, bitrate_kbps, size, mtime)`

Choices and reasons:

- **One album = one folder** (`Artist/Album/`). Tags are only used for display. Grouping by tag splits compilations into dozens of fake albums. The displayed name is the most common album tag of the folder, otherwise the folder name.
- **Track ids are stable**: `INSERT … ON CONFLICT(path) DO UPDATE … RETURNING id` keeps the `id` of a modified track. `/stream/{id}.mp3` URLs and queues stay valid after a rescan.
- **Paths are relative to `MUSIC_ROOT`**, so the library can be moved.
- **Fallback artist**: with no artist tag, the album takes the first folder of the path. Without it, 14 % of the tracks of the test library (classical, soundtracks…) would have no artist.
- **Compilation**: `artist_count > 3` gives `various_artists: true` in the API.
- The database lives on the **SD card** (`~/.local/share/music-server/`) because the library disk is mounted read-only.

### Scan (`scan.rs`)

- Walks with `walkdir`, keeps only `.mp3` files (case-insensitive). `.ini`, `.nfo`, `.m3u`, `.cue`, `.db`… are ignored.
- **First scan: 157 s** for 43,794 files (~280/s through FUSE/NTFS). **Rescan: 2–4 s.**
- Inserts in batches of 1,000 in one transaction, otherwise SQLite on an SD card crawls.
- **Files with unreadable tags** (175 on the first pass: invalid ID3 frames, truncated UTF-16…): the file is still indexed, with the file name as title. The audio properties (duration) are then read without the tags. Files without a duration are retried at every scan. Three remain (probably truncated).
- **Deletion guard**: if the walk finds no MP3 while the index contains some, nothing is deleted and the scan ends in error. An unmounted disk must never empty the database.
- The scan runs in a dedicated thread; `GET /scan` gives the counters (`files_seen`, `indexed`, `unchanged`, `tag_errors`, `removed`, `duration_s`, `error`).

## Seeking inside a track (`mp3seek.rs`, `player.rs`)

HEOS has **no seek command** and the amplifier cannot move forward in a URL it is playing (measured). The server does it instead: it gives the amplifier a URL whose content **starts at the wanted time**, `GET /seek/{id}/{ms}.mp3`, and restarts playback on it.

- **Locating** (`mp3seek::find_offset`): skips the ID3v2 tag, then reads the Layer III frame headers (4 bytes), jumping from frame to frame (`seek_relative` in a 256 KB buffer) and adding their durations until the first frame that starts at or after `ms`. Works for **CBR and VBR** without trusting a missing or wrong Xing table; resynchronises byte by byte over junk; refuses (`400`) a file with no Layer III frame. Measured: within 10–20 ms for CBR and under 0.1 s for VBR, decoding without errors (`ffmpeg`). The very first frame after the cut may glitch for a few milliseconds (bit reservoir).
- **Response**: `200`, `Content-Type: audio/mpeg`, exact `Content-Length`, `Accept-Ranges: none`, body streamed from the byte found. No ID3 tag, no `Range`. Plain ASCII URL, no `?` or `=` (the amplifier decodes nothing in the URL).
- **Player**: `Cmd::Seek(ms)` / `Cmd::SeekBy(delta)` go through `start_at(index, offset, keep_duration)`: same sequence as a track change (stop, clear the queue, `play_stream`), hence **~1 s of silence**, and the fake `stop` events are ignored by `transition`. The amplifier counts its position **from the start of the cut stream**: the server keeps `offset_ms` and publishes `offset_ms + cur_pos`. The duration the amplifier reports for a cut stream is ignored (it is the remaining part): the known total duration is kept. A seek aims at `duration − 1.5 s` at most (beyond that it would just skip to the next track); less than a second from the start means the plain stream. End-of-track detection works unchanged (estimated position = offset + amplifier position): tested, a jump to 3:16 of a 3:23 track chains to the next track.
- Seeking while paused restarts playback.

## Mute

`POST /player/mute {"mute": true|false}` → HEOS `player/set_mute?state=on|off`. The state also comes from the `player_volume_changed` event (`mute` attribute) and from `player/get_mute` on connection; it is published in the snapshot (`mute`). The volume is not modified: unmuting returns to the previous level.

## Search (`search.rs`)

Three FTS5 virtual tables (`fts_tracks`, `fts_albums`, `fts_artists`), tokenizer `unicode61 remove_diacritics 2` (accent- and case-insensitive) and 2- and 3-letter prefix indexes (type-ahead stays fast). They are **derived data** of `tracks` and `albums`: fully rebuilt (3 s for 43,794 tracks on the Pi) when a scan added, changed or removed something, or when their row count does not match the index. A scan without changes does not touch them.

Query: the user's text is split into words (alphanumeric characters only, 8 words at most), each word becomes a quoted prefix term, and all must match (`"bea"* "hel"*`). Since only alphanumeric characters survive, no FTS5 syntax can be injected. `bm25` ranking with weights: title 10, artist 4, album 2 for tracks; name 10, artist 5, folder 3 for albums.

Artists come from `albums.artist` (the folder's main artist), compilations excluded; an artist who only appears on compilation tracks is found through those tracks.

## mDNS (`mdns.rs`)

The server announces itself as the DNS-SD service **`_music-server._tcp`**, instance "Music Server", with TXT records `version`, `api=1` and `path=/`. The `mdns-sd` crate (pure Rust, inside the binary: nothing to install on the Pi).

- **A single interface**: the one that carries the address of `MUSIC_PUBLIC_URL`. The Pi may also have Docker bridges that must never be announced. The host of `MUSIC_PUBLIC_URL` must therefore be an IP address.
- **A distinct host name** (`<hostname>-music.local.`): Avahi, already running on Raspberry Pi OS, owns `<hostname>.local`. Announcing the same name would cause a conflict.
- On shutdown (`SIGINT`/`SIGTERM` through systemd) the server sends the goodbye packets: the service disappears at once for clients (verified with `dns-sd`, `Rmv`).
- An mDNS failure must not prevent the server from starting: the error is logged ("mDNS disabled").

## Covers

**Finding the source** (`covers::refresh`, called at the end of each scan). Priority:

1. **Image in the album folder.** Each `jpg/jpeg/png/webp` file is scored: `cover` (100), `folder` (95), `front` (92), `albumart` (90), `album` (85); names containing those words count a bit less; `large` adds 5 points; `small`, `thumb`, `mini` or `tiny` remove 60; `back`, `rear`, `inlay`, `inside`, `tray`, `cd`, `disc`, `booklet`, `label`, `insert` or `spine` remove 50; a file under 4 KB removes 30; size adds up to 10 points. This covers Windows Media Player leftovers: `AlbumArt_{GUID}_Large.jpg` beats `AlbumArtSmall.jpg`. Hidden files (`._x.jpg`) are ignored.
2. **Disc folder** (`CD1`, `Disc 2`, `Disk_3`, `DVD1`): if nothing is found, the parent folder is checked (`Artist/Album/CD1/…` with the cover in `Artist/Album/`).
3. **Embedded picture** in one of the first 3 MP3 files of the album (the "front cover" picture first, otherwise the first picture).
4. **Nothing** (`none`). Possibly completed by `online.rs`.

The result is stored in `album_covers` (`kind`, `source`, size and date of the source). An album is **resolved again only if it changed**: date of the last audio file (`tracks_stamp`), date of the folder (`dir_mtime`), or a vanished source. A scan with no change costs one `stat` per album folder.

**Thumbnails** (`GET /cover/{id}?size=`): generated on demand then cached on the SD card, `covers/<size>/<album>-<key>.jpg`. The key is an FNV-1a hash of (kind, source, date, source size, requested size): if the image changes, the key changes and the old thumbnail is no longer served. Decoding with the `image` crate (JPEG, PNG, WebP only), `CatmullRom` resize keeping the aspect ratio, never upscaled, JPEG re-encoding at quality 82. Safeguards: 30 MB per source image, 12,000 px per side, 512 MB allocation at most.

Concurrency: **2 generations at a time** (a Pi decoding big files must not starve other services) and one lock per thumbnail, so a grid asking for the same cover twice does the work once.

Failures: an image the decoder refuses turns the album into `none` (clients stop asking for it). A disk access error (disk missing) changes **nothing**: it is transient.

**Online covers** (`online.rs`, only with `COVERS_ONLINE=1`). For `none` albums that have an artist (compilations excluded), every 6 hours and at most once per album per month: MusicBrainz search (`release:"…" AND artist:"…"`); only `score ≥ 95` **and** an identical title after normalisation (lowercase, no punctuation or parentheses) are accepted, then `front-500` is downloaded from the Cover Art Archive: **the `release-group` first** (the reference artwork), then each edition. Observed while testing: without this, a well-known album ended up with a regional edition's cover. At most one MusicBrainz request every 1.1 s. The image is checked (decoded) before being saved in `covers/online/<album>.jpg`. A network failure interrupts the pass without marking the albums as tried.

## Streaming

`GET /stream/{id}.mp3` looks the path up by `id` then delegates to `tower_http::services::ServeFile`, which handles `Range` (`206`), `HEAD`, `Content-Length`, `Last-Modified` and `audio/mpeg`. `Range` is essential: libraries contain continuous mixes of up to 189 MB.

## HEOS client (`heos.rs`)

- One task owns the `TcpStream`. Commands arrive through a channel, **only one in flight at a time** (HEOS answers in order, possibly after an interim `command under process` reply that is ignored).
- `event/...` lines go to a `broadcast` channel (`player_state_changed`, `player_now_playing_progress`, `player_playback_error`, `player_volume_changed`…). The client adds two synthetic events: `connected` and `disconnected`.
- Automatic **reconnection** (2 s, doubling up to 15 s). During an outage `command()` fails immediately ("HEOS not connected"); requests queued at that moment are rejected instead of being replayed later.
- **Discovery**: SSDP (`M-SEARCH`, target `urn:schemas-denon-com:device:ACT-Denon:1`), otherwise `HEOS_HOST`. Verified from the Pi: the amplifier is found in under 2 s.
- Maximum delay per command: 10 s.
- Attribute values are not escaped: our URLs are plain ASCII (`http://<pi>:8765/stream/123.mp3`). See HEOS-FINDINGS section 3 for the encoding pitfall.

## Player (`player.rs`)

An **actor** (one task, one state) receives API commands and HEOS events in a single `select!`. No lock, no race.

State machine: `stopped → loading → playing ⇄ paused`.

Starting a track (`start_track`): `set_play_state stop` → `clear_queue` → `play_stream`. During this sequence `transition` is active and **every `stop` is ignored** (the amplifier emits a fake one on every `play_stream`). `transition` ends when the amplifier says `state=play`, or after 12 s (error "the amplifier never started").

End of track = a `stop` we did not cause, with the estimated position within 3 s of the duration (`is_natural_end`). The position is extrapolated between two progress events (every ~5 s). In that case the next track starts. Otherwise (stopped from the HEOS app for example), the player goes to `stopped` without chaining.

Playback errors: the amplifier repeats the same error several times. It is handled once per 3 s window, the player skips to the next track, and after 3 errors in a row it stops with the error message.

Position and duration: the duration comes from the amplifier's progress event (the most reliable). The `lofty` estimate (based on bitrate) is used before the first event.

## What is deliberately not done

- No native HEOS queue or DLNA: measured, plan B is not needed (HEOS-FINDINGS section 4).
- No thumbnail pre-generation: they are created on first display (to add if the first opening of the grid is slow on the Pi).
- No purge of orphan thumbnails (a changed cover leaves the old thumbnail behind, a few tens of KB).
- No API authentication: local network only.
- No typo-tolerant search (FTS5 only does prefixes): "beatels" finds nothing.
- No disk watching (`notify`): `POST /scan` or a service restart is enough for now.
- No gapless playback between tracks: ~0.3–0.5 s, the limit of `play_stream`.
- No seek without silence: ~1 s (stop, clear the queue, new URL, download by the amplifier).
