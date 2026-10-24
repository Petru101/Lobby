#!/bin/bash
# Renders lobby's window into a PNG inside an invisible headless screen, so no window shows up
# on the desktop: run/snapshot.sh out.png [VAR=value ...]
# (LOBBY_SNAPSHOT_DETAILS=<id>, LOBBY_SNAPSHOT_SEARCH=<text>, LOBBY_SNAPSHOT_FILTER=<n>,
# ADW_DEBUG_COLOR_SCHEME=prefer-dark ...)
# The screen is gamescope's; when its Vulkan won't start (the NVIDIA driver sometimes refuses
# new devices), or with SNAPSHOT_SCREEN=kwin, KWin's virtual one in a D-Bus session of its own
# (so it can't touch the desktop's KWin), with GTK and WebKit drawing in software.
set -o pipefail
dir=$(cd "$(dirname "$0")/.." && pwd)
out=$(realpath -m "$1"); shift
if [ "$SNAPSHOT_SCREEN" != kwin ]; then
    probe=$(host-spawn sh -c 'timeout 20 gamescope --backend headless -W 64 -H 64 -- true 2>&1')
    case "$probe" in *"vkCreateDevice failed"*) SNAPSHOT_SCREEN=kwin ;; esac
fi
if [ "$SNAPSHOT_SCREEN" = kwin ]; then
    session=$(mktemp "${TMPDIR:-/tmp}/lobby-snapshot-XXXXXX.sh")
    {
        echo '#!/bin/sh'
        printf 'exec env GDK_BACKEND=wayland GSK_RENDERER=cairo GTK_A11Y=none WEBKIT_DISABLE_DMABUF_RENDERER=1 WEBKIT_DISABLE_COMPOSITING_MODE=1 LOBBY_SNAPSHOT=%q' "$out"
        printf ' %q' "$@"
        printf ' %q\n' "$dir/target/release/lobby"
    } > "$session"
    chmod +x "$session"
    timeout 150 host-spawn dbus-run-session -- kwin_wayland --virtual --socket "lobby-snapshot-$$" --width 1280 --height 860 --exit-with-session "$session"
    status=$?
    rm -f "$session"
    exit $status
fi
exec timeout 150 host-spawn gamescope --backend headless -W 1280 -H 860 -w 1280 -h 860 -- \
    env GDK_BACKEND=x11 LOBBY_SNAPSHOT="$out" "$@" "$dir/target/release/lobby"
