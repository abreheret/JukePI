#!/usr/bin/env bash
# Runs ON the Raspberry Pi (copied and started by deploy.sh). Installs the new binary and restarts the service.
set -e
mv ~/music-server/music-server.new ~/music-server/music-server
chmod +x ~/music-server/music-server
UNIT=/etc/systemd/system/music-server.service
if [ ! -f "$UNIT" ] || [ "${FORCE_UNIT:-0}" = "1" ]; then
  sudo sh -c "cat /tmp/music-server.service > $UNIT"
  echo "systemd unit installed: EDIT $UNIT (MUSIC_PUBLIC_URL, MUSIC_ROOT...) then: sudo systemctl daemon-reload"
else
  echo "systemd unit already present: left untouched"
fi
rm -f /tmp/music-server.service
if [ -f /tmp/music-server.env ]; then
  sudo install -d -m 755 /etc/music-server
  sudo install -m 600 -o root -g root /tmp/music-server.env /etc/music-server/music-server.env
  sudo install -d -m 755 /etc/systemd/system/music-server.service.d
  sudo install -m 644 /tmp/music-server-envfile.conf /etc/systemd/system/music-server.service.d/10-env-file.conf
  rm -f /tmp/music-server.env
  echo "settings installed: /etc/music-server/music-server.env (root only)"
fi
rm -f /tmp/music-server-envfile.conf
sudo systemctl daemon-reload
sudo systemctl enable music-server
sudo systemctl restart music-server
sleep 3
systemctl is-active music-server
curl -s localhost:8765/health; echo
