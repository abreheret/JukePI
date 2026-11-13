# Guide for writing a client app

🇫🇷 [Français](ANDROID-CLIENT.fr.md)

What a client app needs to know about the server. The full endpoint contract is in the [README](../README.md); here: discovery, conventions and pitfalls. (The reference client is [`../../android-app`](../../android-app/README.md).)

## 1. Finding the server (mDNS)

The server announces itself over DNS-SD: type **`_music-server._tcp`**, instance "Music Server", TXT `version`, `api` (=`1`), `path` (=`/`).

```kotlin
val nsd = context.getSystemService(Context.NSD_SERVICE) as NsdManager
nsd.discoverServices("_music-server._tcp", NsdManager.PROTOCOL_DNS_SD, listener)
// onServiceFound -> register a ServiceInfoCallback (resolveService is deprecated since API 34)
// onServiceUpdated(info) -> http://${info.hostAddresses.first().hostAddress}:${info.port}
```

- Use the resolved **IP address** (`info.hostAddresses`), not the name `<hostname>-music.local`: Android does not always resolve `.local` names outside NSD.
- **Android 17 (API 37)**: for an app targeting API 37, `ACCESS_LOCAL_NETWORK` is mandatory (declared in the manifest **and** requested at run time) before any connection to the local network: TCP to the Pi, mDNS, `.local` names. Without it, connections **time out without any error message**. See <https://developer.android.com/privacy-and-security/local-network-permission>. (`NEARBY_WIFI_DEVICES` was only the interim rule of Android 16.)
- Give `registerServiceInfoCallback` a shared executor that is never shut down: shutting it down crashes the app (`RejectedExecutionException`).
- Provide a manual address entry (`http://192.168.1.50:8765`) for when discovery fails (guest network, Wi-Fi repeater) and remember the last address that worked.
- Check `GET /health` after resolving.
- Tested on a Pixel 8a running Android 17 with the app in `../../android-app`: the server is found automatically.

## 2. API conventions

- JSON, UTF-8. Plain HTTP: add `android:usesCleartextTraffic="true"` (or a `network-security-config` for the local network).
- No authentication: local network only.
- Errors: `{"error": "..."}` with `400`, `404`, `409` or `500`.
- Paginated lists: `{total, limit, offset, items}` (`/albums`, `/artists`). Search returns three lists.
- Stable numeric ids: a `track_id` or `album_id` stays valid after a rescan as long as the file exists.
- The server never reports "no result" as an error: it returns an empty list.

## 3. Screens and endpoints

| Screen | Endpoints |
| --- | --- |
| Home (grid) | `GET /albums?sort=recent\|random\|name&limit=60&offset=`; cover `GET /cover/{id}?size=300` |
| Album | `GET /albums/{id}` (tracks sorted by disc/number); `POST /player/play {"album_id":N,"start":0}` |
| Artists | `GET /artists`; `GET /albums?artist=Name` |
| Search | `GET /search?q=...` on every keystroke (answers in 10–17 ms on the local network), with a ~150 ms debounce in the app |
| Now playing | `GET /player/events` (SSE); `POST /player/pause`, `/resume`, `/next`, `/previous`, `/volume`, `/mute`, `/seek` |

## 4. Covers

- `GET /cover/{album_id}?size=` with `size` ∈ {64, 150, 300, 600}. Grid thumbnail: 300; list: 64 or 150; full screen: 600.
- Albums carry `has_cover` and `cover_path`. **If `has_cover` is false, do not make the request**: show a placeholder. A `404` also means "no cover".
- Thumbnails come with `ETag` and `Cache-Control: max-age=86400`: let Coil / OkHttp cache them.
- The first request for a cover generates the thumbnail (~60 ms per image on the Pi, 2 at a time); later ones come from the disk cache. A grid of 24 covers displayed in 1.5 s cold, 0.16 s afterwards.

## 5. Player and real time

`GET /player/events` is a **Server-Sent Events** stream: each `data:` event holds the full snapshot (`status`, `index`, `current`, `position_ms`, `duration_ms`, `volume`, `mute`, `queue`, `error`, `heos_connected`, `updated_at_ms`).

- **The position is only refreshed every ~5 s** (the amplifier's pace). While `status == "playing"`, the app extrapolates: `position = position_ms + (now - arrival time of the event)`, bounded by `duration_ms`. The phone's and the Pi's clocks may differ: measure the offset when an event arrives instead of comparing absolute times.
- `POST /player/*` commands answer `202` with the state **before** the amplifier reacted: rely on the SSE stream, not on the response.
- Playback errors arrive in `error` (the server moves on to the next track by itself, then stops after 3 errors in a row).
- `status == "loading"` lasts ~0.5 s when the track changes (the amplifier cannot preload): show a loading state, not an error.
- `heos_connected == false`: the amplifier is unreachable; disable the commands.
- **Seeking**: `POST /player/seek {"position_ms":N}` or `{"delta_ms":±N}`. The server restarts the stream at the wanted spot: **~1 s of silence**, during which `status` goes through `loading`. Only send the position when the finger is lifted (not on every slider movement) and keep the chosen value on screen until the next snapshot.
- **Mute**: `POST /player/mute {"mute":bool}`, state in `mute`.
- A 0.3–0.5 s gap between two tracks is normal.
- The volume is 0–100; the amplifier also listens to its remote and to the HEOS app: the volume can change without a command from the app, it is updated through the stream.
- Reconnect the SSE with an increasing delay if the stream closes (service restart, Wi-Fi drop). Use an HTTP client **without a read timeout** for it: the server only sends a keep-alive every ~15 s.

## 5 bis. Playing on the phone and downloads

- The server also serves tracks to the app: `GET /stream/{id}.mp3` (`Range` is supported, which ExoPlayer uses to seek) and `GET /cover/{id}?size=600`. To download an album: `GET /albums/{id}` (tracks), then each `/stream/{id}.mp3`.
- **"Is the server there?" probe**: `GET /health` must answer `{"status":"ok"}`. Check the body, not just the connection: the Pi's address may belong to another device on another network. Short timeouts (1.5 s).
- On the phone, playback and seeking are local: the server's `/player/seek` only concerns the amplifier.

## 5 ter. "More like this" (suggestions)

- `GET /recommendations?track_id=<current>&limit=20&exclude=<queued ids, comma separated>` gives tracks of the library that sound like the current one (see the [README](../README.md)). Each item has a `reason`; `source` says whether Last.fm contributed (`lastfm`) or only the local tags (`local`).
- Append them: amplifier `POST /player/enqueue {"track_ids":[...]}` (the queue must not be empty); phone `addMediaItems`. If playback had ended (status `stopped` on the last track), jump to the first new item.
- **Automatic suggestions** (the reference app does it by default): right after a queue starts, ask once, seeded with the queue's last track, for about 10 tracks and append them, so the user sees what follows and can skip it; ask again when the queue's last track starts. Only once per queue / per track, only when the server is `Online`, and keep the ids you were given (`reason`) so the queue can mark them as suggestions.
- Tell the user where the suggestions come from: with a Last.fm key on the server, the artist and title are sent to Last.fm.
- The call is cheap (cache hit: a few tens of milliseconds; first call about 0.5 s).

## 6. Pitfalls

- **Two outputs**: the amplifier (the server drives it, the app only commands) or the phone itself (the app plays `/stream/{id}.mp3` or a downloaded file). The `/player/*` API only concerns the amplifier.
- Titles, artists and albums come from the files' tags: expect number prefixes (`01. Help!`), spelling variants (`AC/DC`, `Ac-Dc`) and years inside album names. Do not "clean" them in the app without an explicit rule.
- `various_artists: true`: compilation, show "Various artists" rather than the returned main artist.
- The server may be scanning (`GET /scan`, `running: true`): lists fill in as it goes.
