#!/bin/sh
# Called by qBittorrent when a torrent finishes ("Run external program on torrent finished"):
#   /config/scripts/torrent-finished.sh "%F"
# It only records the content path; import-torrent-albums.sh (on the host) does the rest.
# Runs inside the qBittorrent container, where /config is qBittorrent's config folder.
printf '%s\n' "$1" >> /config/finished-torrents.txt
