# Operations

🇫🇷 [Français](OPERATIONS.fr.md)

This guide describes the **reference setup** JukePI was developed on; adapt addresses, disks and users to your own installation. IP addresses are examples.

## The Raspberry Pi (reference setup)

| Item | Value |
| --- | --- |
| Model | Raspberry Pi 4 Model B, 4 GB, 4 cores |
| OS | Debian 13 (trixie), kernel `6.12.x-rpt-rpi-v8`, aarch64, glibc 2.41 |
| Address | 192.168.1.50 (example; reserve it in your router's DHCP: the URL given to the amplifier depends on it) |
| Access | SSH (ideally with a key: `ssh-copy-id`); the user's password is still needed for `sudo` |
| Power | official Raspberry Pi 5.1 V / 3 A supply. `vcgencmd get_throttled` = `0x0` even during a full scan |
| Other services | other services may run on the same Pi; the music server is light (no transcoding) |
| Idle load | load average ~0.2; 3 GB of RAM available |

### Ports

The music server uses **8765**. Port 8080 is often already taken (web UIs of other services): do not use it. Check with `ss -ltn`.

### Disks

| Disk | Mount | Content |
| --- | --- | --- |
| microSD card 29 GB (ext4) | `/` | system, binary, SQLite index |
| another USB disk (ext4) | `/mnt/usb` | other data |
| **1 TB SSD in a USB 3 enclosure, NTFS** (label `SSD`, UUID `XXXXXXXXXXXXXXXX`: replace with yours, `lsblk -f`) | `/mnt/music` | music library, `music/` folder (~290 GB) |

The SSD is NTFS so it can be plugged into a Windows PC and receive the MP3 files directly (Windows does not write ext4). The Pi mounts it **read-only** with `ntfs-3g`. Line added to `/etc/fstab`:

```
UUID=XXXXXXXXXXXXXXXX  /mnt/music  ntfs-3g  ro,uid=1000,gid=1000,umask=022,nofail,x-systemd.device-timeout=10  0  0
```

`nofail`: the Pi boots even without the SSD. Keep a backup of the old file (`/etc/fstab.bak-music`). The mount was tested with `umount` + `mount -a`, **not with a real reboot**.

**Adding music**: plug the SSD into Windows, copy into `music/`, **"Eject" properly** (otherwise the NTFS volume is flagged dirty and mounting may fail or fall back to read-only), plug it back into the Pi, then `curl -X POST http://192.168.1.50:8765/scan`.

### USB power budget

A Pi 4 limits all USB ports together to **1.2 A**, whatever the power supply. With a bus-powered 2.5" disk and the SSD, `get_throttled` stayed at `0x0` and `dmesg` showed no I/O error. Re-check if you add a third USB device (`vcgencmd get_throttled`, `dmesg | grep -iE "under-voltage|usb"`).

### Cache

`~/.cache/music-server/covers/` on the SD card: thumbnails (`64/`, `150/`, `300/`, `600/`) and downloaded covers (`online/`). Order of magnitude: 20–60 KB per 300 px thumbnail, so ~100–200 MB for 3,339 albums if the whole grid was displayed. Safe to empty: everything regenerates (`rm -rf ~/.cache/music-server/covers/{64,150,300,600}`). Do not delete `online/` without reason: downloaded covers exist nowhere else.

## The systemd service

Source file: `deploy/music-server.service`, installed as `/etc/systemd/system/music-server.service`.

- User `pi`, `Nice=10` (leaves priority to other services), `Restart=on-failure`, `RestartSec=5`.
- `After=` and `Wants=mnt-music.mount`: the service starts after the SSD is mounted, **but starts anyway if the SSD is missing** (the scan then ends with the error "music root … is not a directory", the API answers, `/stream` fails). Deliberately no `RequiresMountsFor`.
- Binary: `/home/pi/music-server/music-server`. Index: `/home/pi/.local/share/music-server/index.db`.

```bash
systemctl status music-server
sudo systemctl restart music-server
journalctl -u music-server -f                  # live logs
journalctl -u music-server --since "-10min" -o cat
curl -s localhost:8765/health ; curl -s localhost:8765/scan
```

## mDNS discovery

Check from a Mac: `dns-sd -B _music-server._tcp local.` (list) then `dns-sd -L "Music Server" _music-server._tcp local.` (host, port, TXT). Expected result: `<hostname>-music.local.:8765`, `version=0.3.0 api=1 path=/`. Multicast (UDP 5353) must get through between the phone and the Pi: same local network, no isolated "guest" network. Avahi (usually already present) and the server coexist on port 5353 without conflict.

## Enabling online covers (privacy)

Off by default. Enabling it sends to `musicbrainz.org` and `coverartarchive.org` (which redirects to `archive.org`) the artist name and album name of **every album without a local cover**, plus the Pi's IP. To enable it, add to `deploy/music-server.service`: `Environment=COVERS_ONLINE=1` (and, to be polite to MusicBrainz, `Environment=MUSICBRAINZ_CONTACT=<address or URL>`), then redeploy. Before enabling: `curl localhost:8765/covers` gives the number of `none` albums that would be concerned.

## Enabling Last.fm (privacy)

Everything is off without a key. What each level sends to Last.fm (`ws.audioscrobbler.com`):

| Setting | Enables | Sent to Last.fm |
| --- | --- | --- |
| `LASTFM_API_KEY` | "more like this" | the artist and title of the track a suggestion is asked for; the account name for the listening history if there is one |
| + `LASTFM_SECRET` and a linked account | scrobbling | everything the **amplifier** plays, with the time (the phone's own player is not covered) |

1. Create an application on <https://www.last.fm/api/account/create> (any name). It gives an **API key** and a **shared secret**.
2. Put them in `deploy/music-server.env` (copy `deploy/music-server.env.example`; git ignores it) and deploy: `deploy.sh` installs the file on the Pi as `/etc/music-server/music-server.env`, owner root, mode 600. Do not add them to the unit file or the repository. (By hand: a drop-in `/etc/systemd/system/music-server.service.d/x.conf` with `[Service]` / `Environment=LASTFM_API_KEY=...`, mode 600.)
3. Link your account once, for scrobbling: `MUSIC_SERVER=<pi-ip>:8765 ./scripts/lastfm-link.sh`. It prints a Last.fm page: log in, click "Allow access", come back and press Enter. Your password is typed on last.fm only. The session key is stored in `~/.local/share/music-server/lastfm_session.json` (mode 600) on the Pi. `./scripts/lastfm-link.sh unlink` (or `DELETE /lastfm/session`) forgets it.
4. Check: `curl localhost:8765/lastfm` gives `{"similarity":true,"scrobbling":true,"user":"<you>",...}` (no secret in it), and `journalctl -u music-server | grep -i last.fm` shows `Last.fm ... enabled`. After a listen, `Last.fm: 1 scrobble(s) sent`, and the track appears in your recent tracks.
5. A Last.fm profile is public by default, and so is the history the scrobbles build: change it in your Last.fm privacy settings if you do not want that.

`/lastfm/*` has no authentication, like the rest of the API: keep the server on a trusted network. If a secret leaked (pasted somewhere public), create a new application on Last.fm, replace the values and redeploy.

## Deploying a new version

From your computer, in `music-server/`: `PI=pi@raspberrypi.local ./deploy/deploy.sh` (tests, cross-compilation, upload, restart; asks for the `sudo` password). If `deploy/music-server.env` exists it is installed on the Pi as `/etc/music-server/music-server.env` (root only) and overrides the unit's `Environment=` lines; without it the Pi keeps its settings. Without a Rust toolchain, deploy the binary of a GitHub release instead: `PREBUILT=./music-server-linux-aarch64 PI=pi@raspberrypi.local ./deploy/deploy.sh` (it skips the tests and the build). The binary is built with `cargo zigbuild --release --target aarch64-unknown-linux-gnu.2.36` (`.2.36` sets the minimum glibc version; the Pi has 2.41). The systemd unit is only installed if the Pi does not have one yet (`FORCE_UNIT=1` to overwrite it): it holds your settings. The same steps were run by hand during development; the script itself has not been run end to end. The `sudo` password prompt is interactive: run the script **in a real terminal**.

After the first deployment that brings covers, the scan resolves all albums (one `read_dir` per folder, plus reading a few MP3 files for embedded pictures): allow a few minutes, follow with `curl localhost:8765/covers` (`resolved_last_scan`).

## Building the release files

`./scripts/build-release.sh` (from the repository root) builds the server binary for the Pi and the APK into `dist/` and writes `SHA256SUMS`. The binary is built with the paths of your machine remapped, and the script then scans both files for your user name, home folder, git e-mail, signing password and any exported `LASTFM_*` secret, and stops if it finds one (it names what it found, never the value). Check the files it produces before attaching them to a release: they must not contain anything of yours. It also checks that the APK is signed with the release key and prints the certificate digest.

**The release key** (once, in a Terminal window of your own, so the password never goes through a chat or a log):

```bash
mkdir -p ~/.jukepi && chmod 700 ~/.jukepi
keytool -genkeypair -v -keystore ~/.jukepi/release.jks -alias jukepi -keyalg RSA -keysize 4096 -validity 10000 -dname "CN=JukePI"
chmod 600 ~/.jukepi/release.jks
security add-generic-password -a "$USER" -s jukepi-release-keystore -w    # macOS Keychain: the script reads the password there
```

(Elsewhere than macOS: export `JUKEPI_KEYSTORE_PASSWORD`, and `JUKEPI_KEYSTORE` if the file is not `~/.jukepi/release.jks`.) **Back up `release.jks` and its password** (a password manager, another disk): without them nobody can update the app any more, and it would have to be republished under another name. Never commit them: `*.jks` is git-ignored. Publish the certificate digest (`apksigner verify --print-certs`) so users can check where an APK comes from; it is in the Android README.

## Troubleshooting

| Symptom | Likely cause | Action |
| --- | --- | --- |
| `GET /player`: `heos_connected: false` | amplifier off / other network / SSDP blocked | Put `Environment=HEOS_HOST=192.168.1.60` in the systemd unit; look for `HEOS discovery failed` in the logs |
| `error: HEOS not connected` when playing | same, or Wi-Fi drop of the amplifier | The client reconnects by itself (2–15 s) |
| The amplifier says `state=unknown`, server log: `404` | wrongly encoded URL (double encoding) or unknown id | See HEOS-FINDINGS section 3 |
| Burst of `Unsupported format` errors at the end of a track | the amplifier's internal queue holds old URLs | The player clears the queue before each track; if it comes from a manual test, send `clear_queue` |
| `/scan`: `error: music root … is not a directory` | disk not mounted | `lsblk`, `sudo mount /mnt/music`; if NTFS is "dirty": plug it back into Windows and eject it properly |
| Music cuts out | the amplifier's Wi-Fi | Connect the amplifier with Ethernet (the manufacturer's recommendation) |
| Disk disconnects | USB budget / power | `vcgencmd get_throttled`; powered USB hub for the 2.5" disk |
| `has_cover: false` everywhere after deployment | the cover scan has not finished, or `MUSIC_ROOT` unreadable | `curl localhost:8765/covers`; `journalctl -u music-server | grep covers` |
| A cover is wrong | wrong file picked in the folder | Rename the wanted image to `cover.jpg` in the album folder, then `POST /scan` (the folder date changes, the album is resolved again) |
| Suggestions come only from the local tags (`source: "local"`) | no `LASTFM_API_KEY`, the artist is unknown to Last.fm, or Last.fm is unreachable | `curl localhost:8765/lastfm`; `journalctl -u music-server \| grep -i last.fm` |
| Listens do not appear on Last.fm | no account linked (`scrobbling:false`), a track under 30 s or played less than half, Pi offline | `./scripts/lastfm-link.sh`; queued listens go out by themselves (`scrobble_queue` table) |
| The app does not see the server | multicast blocked (guest network, Wi-Fi repeater), or `MUSIC_PUBLIC_URL` is not an IP | From a Mac: `dns-sd -B _music-server._tcp local.`; log: `mDNS: announcing` or `mDNS disabled`; as a last resort type `http://192.168.1.50:8765` by hand |
| Search finds nothing though the library is indexed | FTS index empty or old | It is rebuilt at start-up if its row count does not match; `journalctl -u music-server \| grep "search index"` |
| Port already in use at start-up | another service on 8765 | `ss -ltn`; change `MUSIC_BIND` |

## Pitfalls met (do not repeat)

1. **`pkill -f pattern` over SSH kills its own session**: the pattern appears in the remote shell's command line (exit code 255, no output). Kill by PID: `for p in $(pgrep -x music-server); do kill $p; done`.
2. **Never send content into `sudo tee` through a wrapper that feeds the password on stdin.** A wrapper that supplies `sudo`'s password on standard input consumes that stdin: `echo line | wrapper tee -a /etc/fstab` wrote the **password** into `/etc/fstab` instead of the line (parse error). Fixed at once from the backup. Safe method: write a file in `/tmp`, then `sudo sh -c "cat /tmp/x > /etc/..."`, then check with `grep`.
3. **Port 8080**: often already taken by other services. Do not use it.
4. **`hostname -I` also returns Docker bridges** (172.17.0.1, 172.18.0.1): the base URL given to the amplifier is in `MUSIC_PUBLIC_URL`, never auto-detected.
5. **NTFS under Windows**: always "Eject" before unplugging the SSD.
6. **`lofty` writes ~30,000 WARN lines** ("Using bitrate to estimate duration") during a full scan. Filtered by default (`lofty=error`).
