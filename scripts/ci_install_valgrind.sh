#!/usr/bin/env bash
# CI: install valgrind from the packages kept in ~/apt-debs (actions/cache), or from the mirror when there are none
# (and keep them for the next run). ci.yml runs it in the background while the workspace builds.
set -euo pipefail
sudo rm -f /var/lib/man-db/auto-update # (no man-db index rebuild after the install)
shopt -s nullglob
debs=(~/apt-debs/*.deb)
if [ ${#debs[@]} -gt 0 ] && sudo apt-get install -y --no-install-recommends "${debs[@]}"; then
  exit 0
fi
sudo apt-get update
sudo apt-get install -y --no-install-recommends valgrind
mkdir -p ~/apt-debs
cp /var/cache/apt/archives/*.deb ~/apt-debs/ || true
