# What was measured on the Denon DRA-900H

🇫🇷 [Français](HEOS-FINDINGS.fr.md)

Everything in this document comes from tests on a real device, in November 2026. The scripts that produced the numbers are in `../experiments/`. The official spec (HEOS CLI v1.16, 2020, unsupported by Sound United) does not cover the DRA-900H: these measurements are the only reliable source.

## 1. The device

| Item | Value |
| --- | --- |
| Model | Denon DRA-900H |
| Firmware | 3.139.173 |
| IP | 192.168.1.60 (DHCP lease; an example address) |
| Network | **Wi-Fi** (`"network": "wifi"`); the Denon manual recommends wired Ethernet for playing from a server |
| HEOS pid | 123456789 (an example value: yours comes from `get_players`) |
| HEOS CLI port | TCP 1255, commands `heos://group/command?attr=value` terminated by `\r\n`, JSON replies |
| Discovery | SSDP `M-SEARCH`, `ST: urn:schemas-denon-com:device:ACT-Denon:1`. The reply carries `LOCATION: http://192.168.1.60:60006/...` |
| HEOS account | not needed for local music |
| Idle volume | 25 |

## 2. `play_stream` works (step 1 of the plan, validated)

`heos://browse/play_stream?pid=X&url=http://192.168.1.50:8765/test.mp3` with an MP3 served by a plain `python3 -m http.server`:

- Reply `success`, then `state=play`.
- The server log shows a `GET` coming from `192.168.1.60`: **the amplifier itself downloads the file** (a single `GET`, no preliminary `HEAD`).
- The server must answer `Content-Type: audio/mpeg`.
- Confirmed by ear on two files (a 192 kbps 8.9 MB test MP3 and a 128 kbps 5.6 MB MP3).
- **Delay between sending `play_stream` and `state=play`: 0.43 to 0.55 s.**
- **What the Denon displays** (title, album, artist in `get_now_playing_media`, and on its front panel) comes from the **ID3 tag at the very start of the stream**, read 3 to 5 seconds after the track starts (`Url Stream` until then). It is not ICY/Shoutcast metadata (the Denon sends no `Icy-MetaData` header; its User-Agent is `GStreamer curlhttpsrc`), nor does it depend on `Accept-Ranges`, `Last-Modified` or HTTP/1.0 vs 1.1. `image_url` is **always empty**: no cover on the amplifier (that would need a DLNA/UPnP media server).
- Many files have **no usable tag** (a Lavf-encoded file had only an encoder name), so the amplifier showed `Url Stream` for them. The server therefore starts every stream with an ID3v2.3 tag built from its index (`id3.rs`): title, artist, album as the app shows them, the file's own tag being skipped.
- **A Xing / Info / VBRI header frame right after the tag stops the tag from being shown** (same file: `Url Stream` for 19 s with the frame, tags after a few seconds without it, and when the stream starts further into the file). The Denon does not need it to play, so the stream skips it (`mp3seek::audio_start`).
- Without that frame the Denon can only **estimate the length** (315 s for a 259 s track), which would break the detection of the end of a track. The exact length is read from the header itself at scan time (`duration_exact` column) and the player trusts it over the Denon's. The same header showed that lofty's estimate was wrong for such files (it read 32 kbps from the header frame: 36 minutes for a 4 minute song).
- The Denon also seems to **remember what it found per URL** (a URL played once without tags kept showing `Url Stream`; adding a query string showed them), so every start uses a fresh `?n=<ms>`.
- Very small files (a 13 s, 325 KB clip) are unreliable: the Denon downloads them whole at once and often shows `Url Stream` anyway. Normal tracks (3 to 12 MB) are fine.
- The duration given by the Denon (`duration=18000`) is more reliable than the `lofty` estimate (18442) for MP3s without a VBR header.

## 3. URL encoding: encode ONCE

Test with `Artist/Album (2016)/07. Artist - Très haut.mp3` (accent, spaces, parentheses):

| URL sent in `play_stream` | What the Denon requests from the Pi | Result |
| --- | --- | --- |
| `.../Album%20%282016%29/07.%20Artist%20-%20Tr%C3%A8s%20haut.mp3` (encoded once) | same | `200`, `state=play` |
| `.../Album%2520%25282016%2529/...Tr%25C3%25A8s...` (double-encoded, as the spec suggests for `%`) | same, `%2520` as is | `404`, `state=unknown` |

The Denon does not decode one level. **Rule:** send the URL as a browser would request it.

The server avoids the problem by serving by id: `http://192.168.1.50:8765/stream/1768.mp3`. No character to encode.

## 4. Chaining two tracks: the measured gap

Test with two 20 s clips (`experiments/chaining_gap.py`). The times come from HEOS events (±0.2 s precision): the gap is measured between the estimated end of track A and the `state=play` event of track B.

| Method | Gap | Errors |
| --- | --- | --- |
| Naive: send the next track when `state=stop` is received | **~0.3 s** | 0 |
| Early: send the next track 0.55 s before the end | ~0.5 s | 0 |
| Early: send the next track 1 s before the end | ~0.55 s | 0 |

Conclusions:

- **No gapless playback** with `play_stream`, but the gap is small.
- **Sending early brings nothing**: `play_stream` during playback cuts the current track then loads the next one, there is no preloading.
- The naive method is enough. It is what `player.rs` implements.
- Plan B of the original design (a DLNA server and the native HEOS queue) is not needed. It might reduce the gap, without proof. To reconsider only if the gap bothers on chained albums (live, concept albums).
- Also measured with the real server on 3 short tracks: chaining detected at `pos 14132 ms / 15000 ms` and `12906 / 13000`, `loading → playing` in ~0.4 s.

## 5. The Denon's internal queue and `clear_queue`

The Denon keeps an internal queue fed by every `play_stream`, and it **advances in it by itself** at the end of a track. Findings:

- Before the queue was cleared, after a clip ended it **asked again for old URLs from earlier tests** (`/old1.mp3`, `/old2.mp3`, by then `404`) in a burst every ~0.5 s. The matching events: `player_playback_error` with `Playback error. Unsupported format.`, `Could not decode the audio stream`, `Unable to play media. Please try again later.`
- **`clear_queue` sent during playback stops the track**: `state=stop` arrives in ~0.1 s (measured).
- `clear_queue` on an already empty queue answers `result: fail`, `eid=4&text=Requested data not available`. Harmless.
- **Every `play_stream` first emits a fake `state=stop`** (+0.14 s) then `state=play` (+0.45 s), plus `player_queue_changed` and `player_now_playing_changed`.
- After `clear_queue` then two `play_stream`, `get_queue` returns `count=1`.

Consequences in `player.rs`:

1. Sequence to start a track: `stop` → `clear_queue` → `play_stream`. The queue is never cleared during playback.
2. Every `stop` is ignored until the `state=play` that follows our own command.
3. Repeated playback errors are deduplicated over 3 s.

## 6. Useful events

Enabled by `heos://system/register_for_change_events?enable=on` (immediate reply).

| Event | Attributes | Use |
| --- | --- | --- |
| `player_state_changed` | `pid`, `state` = `play\|pause\|stop` | state machine |
| `player_now_playing_progress` | `pid`, `cur_pos`, `duration` (in **milliseconds**) | **every ~5 s** (positions 2000, 7000, 12000, 17000…); first value 0 at start |
| `player_now_playing_changed` | `pid` | ignored |
| `player_queue_changed` | `pid` | ignored |
| `player_playback_error` | `pid`, `error` | skip to the next track |
| `player_volume_changed` | `pid`, `level`, `mute` | volume display |

Natural end of track: `state=stop` with no `playback_error`, and no command from us. There is no dedicated "end of track" event.

**There is no seek command** in HEOS CLI, and nothing in the spec to move inside a stream. Workaround implemented on the server: restart `play_stream` on a URL that starts at the wanted time (`/seek/{id}/{ms}.mp3`). Measured: the Denon accepts these cut streams **without a Xing header and without `Range`**, reports its position from the start of the cut stream (hence `offset_ms` on the server) and reports the duration of the cut stream; sound resumes after **~1 s** (command received → first `GET` from the Denon about 0.9 s later, which includes `stop`, `clear_queue` and `play_stream`).

## 7. Other verified commands

| Command | Result |
| --- | --- |
| `player/get_players` | `name`, `pid`, `model`, `version`, `ip`, `network`, `serial` |
| `player/get_play_state` | `state=play\|pause\|stop` (or `unknown` after an error) |
| `player/get_now_playing_media` | `type`, `song`, `album`, `artist`, `image_url` (empty), `qid`, `sid` (1024 for URLs) |
| `player/set_play_state` | `play`, `pause`, `stop` |
| `player/get_volume` | `level` |
| `player/get_queue` | `returned`, `count` |

## 8. Mute

`player/set_mute?pid=X&state=on|off`: `success`, immediate effect (tested: on then off, the state is read back by `player_volume_changed`/`get_mute`). `player/get_mute` returns `state=on|off`. The volume is unchanged.

## 9. Not tested yet

- Playing a 189 MB continuous mix to the end (`Range` is verified, not the long playback).
- Behaviour after the Denon restarts during playback.
- Putting the Denon on Ethernet (the chaining gap is expected to be steadier).
