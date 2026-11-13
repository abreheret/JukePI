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
sudo systemctl daemon-reload
sudo systemctl enable music-server
sudo systemctl restart music-server
sleep 3
systemctl is-active music-server
curl -s localhost:8765/health; echo
