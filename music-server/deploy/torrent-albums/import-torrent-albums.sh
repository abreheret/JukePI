#!/usr/bin/env bash
# Copies finished torrent downloads that contain MP3 files into the music library, then asks the music
# server to rescan. Runs on the host (systemd units next to this file), as the user that owns the library.
#
# qBittorrent records each finished torrent in FINISHED_LIST (see torrent-finished.sh); paths there are
# the ones qBittorrent sees (CONTAINER_DOWNLOADS, e.g. /downloads/...), mapped here to DOWNLOADS.
#
# - A finished folder with at least one .mp3 (any depth) -> DEST_DIR/<folder name>/ : its MP3 files and
#   cover images (jpg, jpeg, png, webp), sub-folders kept (multi-disc albums, discographies).
# - A finished single .mp3 file -> DEST_DIR/_Singles/<file name>.
# - The copy is made in STAGING (same disk as the library, outside it) then moved in one rename, so the
#   music server never indexes a half-copied album.
# - Each torrent is handled once (DONE_LIST): deleting the torrent keeps the music, deleting the album
#   from the library does not bring it back. To import a torrent again, remove its line from DONE_LIST.
# - Nothing happens while one of the disks is not mounted; the entries wait for the next run.
set -euo pipefail

DOWNLOADS="${DOWNLOADS:-/mnt/usb/WD2To/downloads}"
CONTAINER_DOWNLOADS="${CONTAINER_DOWNLOADS:-/downloads}"
FINISHED_LIST="${FINISHED_LIST:-/home/pi/.config/qbittorrent/finished-torrents.txt}"
DONE_LIST="${DONE_LIST:-/home/pi/.local/state/jukepi/imported-torrents.txt}"
DEST_DIR="${DEST_DIR:-/mnt/music/music/_Torrents}"
STAGING="${STAGING:-/mnt/music/.jukepi-incoming}"
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
if ! on_mounted_disk "$(dirname "$DEST_DIR")"; then
  echo "music disk not mounted ($DEST_DIR): nothing to do"
  exit 0
fi
[ -f "$FINISHED_LIST" ] || exit 0
mkdir -p "$DEST_DIR" "$STAGING" "$(dirname "$DONE_LIST")"
touch "$DONE_LIST"
rm -rf "${STAGING:?}"/* # copies cut by a crash or a reboot

imported=0

done_with() { printf '%s\n' "$1" >> "$DONE_LIST"; }

# Free space check: size of what will be copied + 1 GiB margin.
fits() { # bytes
  local avail
  avail="$(df -B1 --output=avail "$DEST_DIR" | tail -1 | tr -d ' ')"
  [ "$avail" -gt $(( $1 + 1024 * 1024 * 1024 )) ]
}

copy_album() { # source_dir name
  local src="$1" name="$2" bytes stage dest
  dest="$DEST_DIR/$name"
  if [ -e "$dest" ]; then
    echo "already in the library, skipped: $name"
    return 0
  fi
  bytes="$(find "$src" -type f \( -iname '*.mp3' -o -iname '*.jpg' -o -iname '*.jpeg' -o -iname '*.png' -o -iname '*.webp' \) -printf '%s\n' | awk '{s+=$1} END {print s+0}')"
  if ! fits "$bytes"; then
    echo "WARNING: not enough space on the music disk for $name ($bytes bytes), will retry" >&2
    return 1
  fi
  stage="$STAGING/$name"
  # (set -e does not apply inside a function called with ||: every step is checked by hand)
  if ! mkdir -p "$stage" || ! rsync -r -t --prune-empty-dirs \
    --include='*/' --include='*.[mM][pP]3' \
    --include='*.[jJ][pP][gG]' --include='*.[jJ][pP][eE][gG]' --include='*.[pP][nN][gG]' --include='*.[wW][eE][bB][pP]' \
    --exclude='*' "$src/" "$stage/"; then
    echo "WARNING: copy of $name failed, will retry" >&2
    rm -rf "$stage"
    return 1
  fi
  if ! mv "$stage" "$dest"; then
    echo "WARNING: could not move $name into the library, will retry" >&2
    rm -rf "$stage"
    return 1
  fi
  echo "imported: $name ($(find "$dest" -type f -iname '*.mp3' | wc -l) MP3, $(( bytes / 1024 / 1024 )) MB)"
  imported=1
}

while IFS= read -r path || [ -n "$path" ]; do
  [ -n "$path" ] || continue
  grep -qxF -- "$path" "$DONE_LIST" && continue
  case "$path" in
    "$CONTAINER_DOWNLOADS"/*) host="$DOWNLOADS/${path#"$CONTAINER_DOWNLOADS"/}" ;;
    *) host="$path" ;;
  esac
  host="${host%/}"
  name="$(basename "$host")"
  if [ -d "$host" ]; then
    if [ -n "$(find "$host" -type f -iname '*.mp3' -print -quit 2>/dev/null)" ]; then
      copy_album "$host" "$name" || continue # no space: keep it for the next run
    fi
    done_with "$path"
  elif [ -f "$host" ]; then
    case "${name,,}" in
      *.mp3)
        if [ ! -e "$DEST_DIR/_Singles/$name" ]; then
          if ! fits "$(stat -c %s "$host")"; then
            echo "WARNING: not enough space on the music disk for $name, will retry" >&2
            continue
          fi
          mkdir -p "$DEST_DIR/_Singles"
          if cp -p "$host" "$STAGING/$name" && mv "$STAGING/$name" "$DEST_DIR/_Singles/$name"; then
            echo "imported single: $name"
            imported=1
          else
            echo "WARNING: copy of $name failed, will retry" >&2
            rm -f "$STAGING/$name"
            continue
          fi
        fi
        ;;
    esac
    done_with "$path"
  else
    done_with "$path" # deleted before we got to it
  fi
done < "$FINISHED_LIST"

if [ "$imported" = 1 ]; then
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
  echo "WARNING: music server still busy after 2 minutes: the new albums show up at its next scan" >&2
fi
