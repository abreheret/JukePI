#!/usr/bin/env bash
# Link your Last.fm account to the server, once, so what the amplifier plays is scrobbled.
#
#   MUSIC_SERVER=192.168.1.50:8765 ./scripts/lastfm-link.sh        # link
#   MUSIC_SERVER=192.168.1.50:8765 ./scripts/lastfm-link.sh unlink # forget the account
#
# Needs LASTFM_API_KEY and LASTFM_SECRET in the server settings (deploy/music-server.env.example).
# Your Last.fm password is typed on last.fm only: neither this script nor the server ever sees it.
set -euo pipefail
S="http://${MUSIC_SERVER:?set MUSIC_SERVER=host:port of the music server, e.g. 192.168.1.50:8765}"
field() { python3 -c 'import sys,json; d=json.load(sys.stdin); print(d.get(sys.argv[1], d.get("error", "")) if sys.argv[1] in d or "error" in d else "")' "$1"; }

if [ "${1:-}" = "unlink" ]; then
  curl -sS -X DELETE "$S/lastfm/session" -o /dev/null -w "unlinked (HTTP %{http_code})\n"
  exit 0
fi

st=$(curl -sS "$S/lastfm")
if [ "$(echo "$st" | python3 -c 'import sys,json; print(json.load(sys.stdin)["scrobbling"])')" = "True" ]; then
  echo "Already linked to account: $(echo "$st" | field user)  (use 'unlink' to change it)"
  exit 0
fi
if [ "$(echo "$st" | python3 -c 'import sys,json; print(json.load(sys.stdin)["similarity"])')" != "True" ]; then
  echo "The server has no LASTFM_API_KEY: see deploy/music-server.env.example" >&2; exit 1
fi
start=$(curl -sS -X POST "$S/lastfm/auth/start")
url=$(echo "$start" | field url)
case "$url" in https://*) ;; *) echo "Cannot start: $url" >&2; exit 1;; esac
echo
echo "1. Open this page, log in to Last.fm and click \"Allow access\":"
echo
echo "   $url"
echo
read -r -p "2. Press Enter once it is done... " _
done_=$(curl -sS -X POST "$S/lastfm/auth/finish")
user=$(echo "$done_" | field user)
if [ "$(echo "$done_" | field scrobbling)" = "True" ]; then
  echo "Linked to Last.fm account: $user. Listens on the amplifier are now scrobbled."
else
  echo "Not linked: $(echo "$done_" | field error)" >&2; exit 1
fi
