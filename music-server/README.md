# JukePI server (`music-server`)

🇫🇷 [Français](README.fr.md)

A music server for the Raspberry Pi. It indexes an MP3 library and drives a **Denon HEOS** amplifier through the **HEOS CLI** protocol (developed and tested on a Denon DRA-900H). The amplifier downloads the MP3 files itself over HTTP from the Pi: no audio goes through the phone.

It is the server half of JukePI; the Android remote is in [`../android-app`](../android-app/README.md).

## Status

| Component | Status |
| --- | --- |
| Scan + SQLite index (tested on 43,794 tracks / 3,339 albums) | done, tested on a real library |
| Albums / artists / tracks / stream API (Range requests) | done |
| HEOS client (connection, SSDP discovery, events, reconnection) | done, tested on a real Denon |
| Player (queue, automatic chaining, pause, next, volume, mute) | done, tested on a real Denon |
| Seeking inside a track (`/player/seek`) | done, tested on a real Denon |
| Covers (folder image, embedded picture, cached thumbnails) | done, tested |
| Online cover lookup (MusicBrainz / Cover Art Archive) | done, **off by default**, tested with a public album |
| Full-text search (titles, albums, artists; accent-insensitive, prefixes) | done, tested (10–17 ms) |
| mDNS announcement (`_music-server._tcp`) | done, verified |
| systemd service | done |
| Recommendations ("more like this"): Last.fm similarity + local genres, lifted by your listening history | done, tested on the real library; the automatic chaining at the end of an album is not yet checked by a full listen |
| Scrobbling of what the amplifier plays to Last.fm | done, account linked; first scrobble not yet checked on the profile |

## Architecture at a glance

```
 Phone ──HTTP/JSON──▶  Pi: music-server (Rust, port 8765)
                          │  SQLite (index)          │ HEOS CLI (TCP 1255)
                          │  music library (disk)    ▼
                          │                       Denon HEOS amplifier
                          └──── GET /stream/{id}.mp3 ◀──┘  (the amplifier downloads the MP3)
```

Details: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Configuration (environment variables)

| Variable | Default | Purpose |
| --- | --- | --- |
| `MUSIC_ROOT` | `/mnt/music/music` | Library root (read-only is enough) |
| `MUSIC_DB` | `~/.local/share/music-server/index.db` | SQLite index (put it on the SD card if the library disk is mounted read-only) |
| `MUSIC_BIND` | `0.0.0.0:8765` | Listen address |
| `MUSIC_PUBLIC_URL` | `http://192.168.1.50:8765` | **Set this to your Pi's address.** Base URL the **amplifier** uses to reach the Pi. Fixed IP, never auto-detected (`hostname -I` also lists Docker bridges) |
| `HEOS_HOST` | (empty: SSDP discovery) | Fixed IP of the amplifier if discovery does not work |
| `HEOS_PLAYER_NAME` | (empty: first player) | Substring of the HEOS player name to drive |
| `MUSIC_CACHE_DIR` | `~/.cache/music-server` | Thumbnail cache (`covers/<size>/`) and downloaded covers (`covers/online/`) |
| `COVERS_ONLINE` | (unset) | `1` enables the online cover lookup. **Sends artist and album names of albums without a local cover to musicbrainz.org and coverartarchive.org** |
| `MUSICBRAINZ_CONTACT` | (empty) | Contact address or URL added to the User-Agent (MusicBrainz asks for one; fill in your own) |
| `COVERS_ONLINE_DELAY_S` | `60` | Delay before the first online lookup |
| `LASTFM_API_KEY` | (unset) | Enables the Last.fm similarity. **Sends the artist and title of the track a suggestion is asked for to ws.audioscrobbler.com.** Keep it in a systemd drop-in, never in the repository |
| `LASTFM_SECRET` | (unset) | Shared secret of your Last.fm application: needed (with a linked account) to scrobble. Same care as the key |
| `LASTFM_USER` | (unset) | Account whose listening history lifts the suggestions when no account is linked; once one is linked its name is used |
| `MUSIC_MDNS` | `1` | `0` disables the mDNS announcement |
| `MUSIC_MDNS_NAME` | `Music Server` | Instance name shown in the app |
| `MUSIC_NO_SCAN` | (unset) | If set, no scan at start-up |
| `RUST_LOG` | `info,lofty=error` | Log level (`lofty` is very chatty) |

The IP addresses in this repository (`192.168.1.50` for the Pi, `192.168.1.60` for the amplifier) are **examples**: use your own.

## API

All responses are JSON. No authentication: trusted local network only.

### Library

| Request | Response |
| --- | --- |
| `GET /health` | `{"status":"ok"}` |
| `GET /albums?limit=50&offset=0&sort=name\|artist\|recent\|random&q=text&artist=name` | `{total, limit, offset, items:[{id,name,artist,various_artists,year,track_count,has_cover,cover_path}]}` |
| `GET /albums/{id}` | `{album, tracks:[{id,title,artist,genre,track_no,disc_no,year,duration_ms,stream_path}]}` |
| `GET /artists` | `{total, items:[{name, album_count}]}` (compilations excluded) |
| `GET /tracks/{id}` | one track |
| `GET /search?q=text&limit=10&types=albums,artists,tracks` | full-text search: `{query, albums, artists, tracks}`. Accent- and case-insensitive, every word is a prefix (`bea hel` finds *The Beatles – Help!*), ranked by relevance (title weighs more than artist, which weighs more than album). `limit` per section: 1–50 (default 10). Tracks also carry `album_name` |
| `GET /cover/{album_id}?size=300` | JPEG thumbnail. `size` ∈ {64, 150, 300, 600} (default 300); aspect ratio kept, never upscaled; `ETag` + `Cache-Control: max-age=86400` (`304` on `If-None-Match`); `404` when the album has no cover |
| `GET /covers` | statistics: albums by cover origin (`file`, `embedded`, `online`, `none`) |
| `GET /stream/{id}.mp3` | the MP3 file: `audio/mpeg`, `Accept-Ranges: bytes`, `206` on `Range` |
| `GET /seek/{id}/{ms}.mp3` | the same track **starting at `ms`** (the response begins at the nearest MP3 frame, at most 26 ms later). Used to seek inside a track; no `Range` |
| `GET /scan` | state of the last scan |
| `POST /scan` | start a scan (`202`, or `409` if one is running) |

Tracks are served **by numeric id**: every URL handed to the amplifier is plain ASCII (see why in [docs/HEOS-FINDINGS.md](docs/HEOS-FINDINGS.md), section 3).

### Recommendations and Last.fm

| Request | Effect |
| --- | --- |
| `GET /recommendations?track_id=N&limit=20&exclude=1,2` | tracks of the library that sound like track `N`: `{seed_track_id, source: "lastfm"\|"local", lastfm_enabled, taste, items}`. Each item is a track plus `reason` (`lastfm_track`, `lastfm_artist`, `same_genre`, `same_artist`). `exclude`: ids already queued. `limit` 1–100 (default 20). Never fails because of Last.fm: without a key it answers from the local tags |
| `GET /lastfm` | `{similarity, scrobbling, user, needs_secret, auth_pending}`. Never contains a key, a secret or a session |
| `POST /lastfm/auth/start` | link an account, step 1: returns `{url}` to open in a browser and authorise. Needs `LASTFM_API_KEY` and `LASTFM_SECRET` |
| `POST /lastfm/auth/finish` | step 2, once authorised: stores the session, scrobbling starts |
| `DELETE /lastfm/session` | unlink the account (`204`) |

How the suggestions are built is described in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#recommendations-and-lastfm); how to set Last.fm up in [docs/OPERATIONS.md](docs/OPERATIONS.md#enabling-lastfm-privacy). Like the rest of the API, `/lastfm/*` has **no authentication**: anyone on the network can unlink the account.

### Player (amplifier)

| Request | Effect |
| --- | --- |
| `GET /player` | snapshot: `status` (`stopped\|loading\|playing\|paused`), `index`, `current`, `position_ms`, `duration_ms`, `volume`, `mute`, `queue`, `error`, `heos_connected`, `updated_at_ms` |
| `GET /player/events` | SSE stream: the full snapshot now, then on every change. The position is only refreshed every ~5 s: clients extrapolate from `updated_at_ms` |
| `POST /player/play` | body `{"album_id":N}` or `{"track_id":N}` or `{"track_ids":[...],"start":0}`: replaces the queue and starts playing |
| `POST /player/next`, `/previous`, `/pause`, `/resume`, `/stop` | commands (`202` + snapshot) |
| `POST /player/jump` | body `{"index":N}` |
| `POST /player/enqueue` | body `{"track_ids":[...]}`: appends to the end of the queue without touching the current track (`400` if nothing is queued: use `/player/play`) |
| `POST /player/volume` | body `{"level":0-100}` |
| `POST /player/mute` | body `{"mute":true\|false}` (the volume is kept) |
| `POST /player/seek` | body `{"position_ms":N}` (absolute) or `{"delta_ms":±N}` (relative). **About 1 s of silence**: the stream is restarted at the wanted spot |

Commands are asynchronous: the `202` response carries the state *before* the amplifier reacted. Errors show up in the snapshot's `error` field.

Example:

```bash
curl -s -X POST http://192.168.1.50:8765/player/play -H 'Content-Type: application/json' -d '{"album_id":131}'
curl -s http://192.168.1.50:8765/player | python3 -m json.tool
curl -N http://192.168.1.50:8765/player/events      # live stream
curl -s -X POST http://192.168.1.50:8765/player/volume -H 'Content-Type: application/json' -d '{"level":20}'
```

## Develop

```bash
cargo test                 # HEOS attribute parser, fake TCP amplifier, MP3 frame seeking, search, covers...
cargo run                  # locally; MUSIC_ROOT=/path/to/test/music MUSIC_DB=/tmp/idx.db MUSIC_BIND=127.0.0.1:18765
PI=pi@raspberrypi.local ./deploy/deploy.sh   # cross-compile for the Pi, upload, restart the service
```

Cross-compiling prerequisites: `brew install zig`, `cargo install cargo-zigbuild --locked`, `rustup target add aarch64-unknown-linux-gnu`.

## Documentation

| File | Content |
| --- | --- |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Modules, data flow, SQLite schema, decisions and their reasons |
| [docs/HEOS-FINDINGS.md](docs/HEOS-FINDINGS.md) | Everything measured on a real Denon (with numbers) |
| [docs/ANDROID-CLIENT.md](docs/ANDROID-CLIENT.md) | Guide for writing a client app: mDNS discovery, API contract, live stream, covers |
| [docs/OPERATIONS.md](docs/OPERATIONS.md) | The Pi, the disk, the service, deployment, troubleshooting, pitfalls |
| [docs/NOTES.md](docs/NOTES.md) | Development notes: key decisions, lessons learned, roadmap |
| `experiments/` | Python scripts that produced the measurements (reproducible) |
| `scripts/test_player.py` | End-to-end player test against a real amplifier |
