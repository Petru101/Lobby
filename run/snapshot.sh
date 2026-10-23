#!/bin/bash
# Renders lobby's window into a PNG inside an invisible headless screen (gamescope), so no
# window shows up on the desktop: run/snapshot.sh out.png [VAR=value ...]
# (LOBBY_SNAPSHOT_DETAILS=<id>, LOBBY_SNAPSHOT_SEARCH=<text>, LOBBY_SNAPSHOT_FILTER=<n>,
# ADW_DEBUG_COLOR_SCHEME=prefer-dark)
set -o pipefail
dir=$(cd "$(dirname "$0")/.." && pwd)
out=$(realpath -m "$1"); shift
exec timeout 150 host-spawn gamescope --backend headless -W 1280 -H 860 -w 1280 -h 860 -- \
    env GDK_BACKEND=x11 LOBBY_SNAPSHOT="$out" "$@" "$dir/target/release/lobby"
