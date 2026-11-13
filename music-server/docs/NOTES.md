# Development notes

🇫🇷 [Français](NOTES.fr.md)

Key decisions, lessons learned and roadmap of the server. The detailed measurements are in [HEOS-FINDINGS.md](HEOS-FINDINGS.md), the design in [ARCHITECTURE.md](ARCHITECTURE.md).

## Key decisions

| Decision | Why |
| --- | --- |
| The amplifier downloads the MP3 files itself (`play_stream` with an HTTP URL); nothing goes through the phone | Validated first with a 10-minute manual test (a plain `http.server` + a telnet session on port 1255) before writing any code. No DLNA server needed |
| Tracks are served **by numeric id** (`/stream/{id}.mp3`) | Every URL handed to the amplifier is plain ASCII: the amplifier decodes nothing in a URL (a double-encoded URL gave a `404`) |
| **One album = one folder** | Grouping by tag splits compilations into dozens of fake albums |
| Stable track ids (upsert keyed by path) | URLs and queues survive a rescan |
| The player is a single **actor** | No lock, no race between API commands and amplifier events |
| `play_stream` + our own queue, chaining on the `stop` event | The amplifier has no usable native queue for URLs; sending the next track early brings nothing (measured) |
| **Seek by serving the track from a given MP3 frame** | HEOS has no seek command; the cut stream is accepted by the amplifier without header or `Range` |
| FTS5 index rebuilt from the tables after a scan that changed something | Simple and fast enough (3 s for 43,794 tracks on a Pi 4) |
| Online cover lookup is **opt-in** | It sends artist and album names to third parties |
| The index lives on the SD card, the library disk is read-only | A disk unplugged or "dirty" must never be able to corrupt anything |
| Recommendations come from **Last.fm similarity filtered by the library**, with the local tags as a fallback | The library is the only thing that can be played, so whatever Last.fm suggests and the library lacks is useless; a local fallback keeps the feature working without a key or offline |
| Last.fm is **opt-in at two levels** (key: suggestions; key + secret + linked account: scrobbling) and the account is linked in the browser | Nothing is sent without configuration, and the Last.fm password never reaches the server |
| Secrets live in a root-only file on the Pi, installed from a git-ignored local file | The repository can be public without ever holding a key |
| Cross-compile with `zig` + `cargo-zigbuild` | The Pi is better left to its other jobs than to compile |

## Lessons learned

- **The amplifier's internal queue wraps around.** After a clip ended, the Denon replayed URLs from earlier experiments. Clearing the queue before each track fixes it, but `clear_queue` during playback stops the track, and every `play_stream` emits a fake `stop`: the player ignores `stop` events while a transition it started is in flight.
- **What the amplifier displays is what the stream's own ID3 tag says**, read a few seconds after the start, and a Xing/Info header frame in front of the audio stops it from being shown. Two fixes with a price: the server builds the tag from its index, and drops the header frame, which makes the amplifier's length estimate unreliable, hence the exact length read from the header (`duration_exact`). Beware: the observation is noisy, only repeat measurements on normal-sized files mean something.
- **Many real-world MP3s have no Xing/VBRI header.** Durations are then only estimates, and some players (ExoPlayer) refuse to seek unless told to assume a constant bitrate. The seek code walks the MP3 frames itself and does not trust any table.
- **`ffprobe` guesses the duration of a cut VBR file from its size**: measure the decoded duration instead.
- **An unmounted disk must not empty the index**: the scan refuses to delete when it finds no MP3 at all.
- **Tag quality is what it is**: 175 of 43,794 files had unreadable tags (invalid ID3 frames, truncated UTF-16); they are still indexed, with the file name as title. 14 % had no artist tag: the folder name is the fallback.
- **Folder images are far more common than embedded pictures** (about 85 % vs 11 % of albums in the test library), and Windows Media Player leftovers (`AlbumArt_{GUID}_Large.jpg`, `AlbumArtSmall.jpg`) need a scoring rule to pick the right file.
- **MusicBrainz returns many editions of a release**: the release-group cover is the right default (a regional edition's cover was picked otherwise).
- Operational pitfalls (SSH, `sudo`, ports, Docker bridges, NTFS) are listed in [OPERATIONS.md](OPERATIONS.md#pitfalls-met-do-not-repeat).

## Numbers (reference setup: Pi 4, 4 GB, USB 3 SSD)

| Measure | Value |
| --- | --- |
| First scan, 43,794 MP3 through FUSE/NTFS | 157 s |
| Rescan with no change | 2–4 s |
| Search index rebuild | 3 s |
| Search request | 10–17 ms |
| Cover resolution for 3,339 albums | ~11 s |
| First 24 thumbnails (cold, 6 requests in parallel) | 1.5 s |
| Gap between two tracks | 0.3–0.5 s |
| Seek (command → sound) | ~1 s |
| `play_stream` → `state=play` | 0.43–0.55 s |

## Roadmap

- [x] Recommendations ("more like this"): Last.fm similarity + local tags + listening history. [ ] Audio embeddings later.
- [ ] Not yet checked by a real listen: scrobbles appearing on the profile, the automatic chaining at the end of an album.
- [ ] Scrobble what the phone plays too (the app would have to report its listens to the server).
- [x] Typo-tolerant search and merged artist spellings ("Queen"/"QUEEN", "AC/DC"/"Ac-Dc").
- [ ] Pre-generate 300 px thumbnails after a scan if the first opening of the grid is slow.
- [ ] Disk watching (`notify`) instead of `POST /scan`.
- [ ] Optional authentication if the API ever leaves the local network.
- [ ] A real reboot test of the `fstab` + service start-up chain; a full end-to-end run of `deploy/deploy.sh`.
- [ ] Untested: playing a 189 MB continuous mix to the end; behaviour after the amplifier restarts mid-playback; the amplifier on Ethernet.
