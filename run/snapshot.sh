#!/bin/bash
# Renders lobby's window into a PNG inside an invisible headless screen, so no window shows up
# on the desktop: run/snapshot.sh out.png [VAR=value ...]
# (LOBBY_SNAPSHOT_DETAILS=<id>, LOBBY_SNAPSHOT_SEARCH=<text>, LOBBY_SNAPSHOT_FILTER=<n>,
# ADW_DEBUG_COLOR_SCHEME=prefer-dark ...); LOBBY_BIN=<program> renders another (an AppImage).
# The screen is gamescope's; when its Vulkan won't start (the NVIDIA driver sometimes refuses
# new devices), or with SNAPSHOT_SCREEN=kwin, KWin's virtual one in a D-Bus session of its own
# and a runtime folder of its own (so it can't touch the desktop's KWin, sockets or portal
# mounts), with GTK and WebKit drawing in software.
set -o pipefail
dir=$(cd "$(dirname "$0")/.." && pwd)
out=$(realpath -m "$1"); shift
if [ "$SNAPSHOT_SCREEN" != kwin ]; then
    probe=$(host-spawn sh -c 'timeout 20 setpriv --no-new-privs gamescope --backend headless -W 64 -H 64 -- true 2>&1')
    case "$probe" in *"vkCreateDevice failed"*) SNAPSHOT_SCREEN=kwin ;; esac
fi
if [ "$SNAPSHOT_SCREEN" = kwin ]; then
    session=$(mktemp "${TMPDIR:-/tmp}/lobby-snapshot-XXXXXX.sh")
    {
        echo '#!/bin/sh'
        printf 'exec env GDK_BACKEND=wayland GSK_RENDERER=cairo GTK_A11Y=none WEBKIT_DISABLE_DMABUF_RENDERER=1 WEBKIT_DISABLE_COMPOSITING_MODE=1 LOBBY_SNAPSHOT=%q' "$out"
        [ $# -gt 0 ] && printf ' %q' "$@"
        printf ' %q\n' "${LOBBY_BIN:-$dir/target/release/lobby}"
    } > "$session"
    chmod +x "$session"
    # A runtime folder of its own too: portals started in that session mount and unmount there
    # (sharing the desktop's, its document portal unmounted the desktop's /run/user/1000/doc
    # on exit once).
    runtime=$(mktemp -d "${TMPDIR:-/tmp}/lobby-snapshot-runtime-XXXXXX")
    chmod 700 "$runtime"
    timeout 150 host-spawn env XDG_RUNTIME_DIR="$runtime" dbus-run-session -- kwin_wayland --virtual --socket lobby-snapshot --width 1280 --height 860 --exit-with-session "$session"
    status=$?
    host-spawn sh -c "fusermount3 -u '$runtime/doc' 2>/dev/null; fusermount -u '$runtime/doc' 2>/dev/null; true"
    rm -rf "$session" "$runtime"
    exit $status
fi
exec timeout 150 host-spawn setpriv --no-new-privs gamescope --backend headless -W 1280 -H 860 -w 1280 -h 860 -- \
    env GDK_BACKEND=x11 LOBBY_SNAPSHOT="$out" "$@" "${LOBBY_BIN:-$dir/target/release/lobby}"
