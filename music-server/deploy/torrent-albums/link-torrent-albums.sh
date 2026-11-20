#!/usr/bin/env bash
# Links finished torrent downloads that contain MP3 files into the music library, then asks the music
# server to rescan. Runs on the host (systemd units next to this file), as the user that owns the library.
#
# qBittorrent records each finished torrent in FINISHED_LIST (see torrent-finished.sh); paths there are
# the ones qBittorrent sees (CONTAINER_DOWNLOADS, e.g. /downloads/...), mapped here to DOWNLOADS.
#
# - A finished folder with at least one .mp3 (any depth) -> LINK_DIR/<folder name> (symlink).
# - A finished single .mp3 file -> LINK_DIR/_Singles/<file name> (symlink).
# - Links whose target is gone (torrent deleted with its files) are removed, but only when both disks are
#   mounted: an unplugged download disk must not empty the library.
# - The music server rescans only when something changed.
set -euo pipefail

DOWNLOADS="${DOWNLOADS:-/mnt/usb/WD2To/downloads}"
CONTAINER_DOWNLOADS="${CONTAINER_DOWNLOADS:-/downloads}"
FINISHED_LIST="${FINISHED_LIST:-/home/pi/.config/qbittorrent/finished-torrents.txt}"
LINK_DIR="${LINK_DIR:-/mnt/music/music/_Torrents}"
SCAN_URL="${SCAN_URL:-http://localhost:8765/scan}"

# True when PATH sits on a mounted disk, not on the root filesystem (SD card) under an empty mount point.
on_mounted_disk() {
  local target
  target="$(findmnt -n -o TARGET -T "$1" 2>/dev/null || true)"
  [ -n "$target" ] && [ "$target" != "/" ]
}

if [ ! -d "$DOWNLOADS" ] || ! on_mounted_disk "$DOWNLOADS"; then
  echo "downloads disk not mounted ($DOWNLOADS): nothing to do"
  exit 0
fi
if ! on_mounted_disk "$(dirname "$LINK_DIR")"; then
  echo "music disk not mounted ($LINK_DIR): nothing to do"
  exit 0
fi
mkdir -p "$LINK_DIR"

changed=0

link() { # target link_path
  if [ -e "$2" ] || [ -L "$2" ]; then
    return 0
  fi
  mkdir -p "$(dirname "$2")"
  ln -s "$1" "$2"
  echo "linked: ${2#"$LINK_DIR"/} -> $1"
  changed=1
}

if [ -f "$FINISHED_LIST" ]; then
  while IFS= read -r path || [ -n "$path" ]; do
    [ -n "$path" ] || continue
    case "$path" in
      "$CONTAINER_DOWNLOADS"/*) host="$DOWNLOADS/${path#"$CONTAINER_DOWNLOADS"/}" ;;
      *) host="$path" ;;
    esac
    host="${host%/}"
    name="$(basename "$host")"
    if [ -d "$host" ]; then
      if [ -n "$(find "$host" -type f -iname '*.mp3' -print -quit 2>/dev/null)" ]; then
        link "$host" "$LINK_DIR/$name"
      fi
    elif [ -f "$host" ]; then
      case "${name,,}" in
        *.mp3) link "$host" "$LINK_DIR/_Singles/$name" ;;
      esac
    fi
  done < "$FINISHED_LIST"
fi

# Links to deleted downloads.
while IFS= read -r -d '' l; do
  if [ ! -e "$l" ]; then
    rm -f -- "$l"
    echo "removed (download deleted): ${l#"$LINK_DIR"/}"
    changed=1
  fi
done < <(find "$LINK_DIR" -type l -print0)

if [ "$changed" = 1 ]; then
  # 202 = scan started. 409 = a scan is already running, but it may have started before this change:
  # wait for it and start a new one (every 5 s, at most 2 minutes).
  for _ in $(seq 24); do
    code="$(curl -s -o /dev/null -w '%{http_code}' -m 10 -X POST "$SCAN_URL" || true)"
    case "$code" in
      202) echo "music server rescan started"; exit 0 ;;
      409) sleep 5 ;;
      *) echo "WARNING: music server at $SCAN_URL answered '$code'" >&2; exit 0 ;;
    esac
  done
  echo "WARNING: music server still busy after 2 minutes: this change shows up at its next scan" >&2
fi
