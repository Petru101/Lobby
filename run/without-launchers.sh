#!/bin/bash
# Checks the standing rule: Lobby's games keep everything with every other launcher gone. Runs on
# the host (host-spawn). Nothing real changes: the disk is read-only, each folder of the home a
# throwaway layer, with no network (so no store sign-in gets renewed) and no session bus.
#   run/without-launchers.sh [check]     every game, with the launchers there and then gone
#   run/without-launchers.sh in <cmd>    runs a command with the launchers gone
#   run/without-launchers.sh play <id>   really starts a game with them gone, on an invisible
#                                        screen with no sound, and lists what's running after $WAIT s (45)
#   PROTON=<name> plays it with that Proton (the choice is thrown away with the view).
#   DURING=<shell command> runs alongside the game on its screen ($DISPLAY, $OUT for results).
#   SCREEN=gamescope: a headless gamescope instead of KWin's invisible screen.
#   GONE=no plays it with the launchers there, to tell a launcher's absence from a game's fault.
#   ONLY=<game id> looks at just that game; ALSO_GONE=<path in the home> removes more, to prove
#   the check catches a loss (ALSO_GONE=.local/share/Terraria ONLY=terraria reports its saves).
set -u
dir=$(cd "$(dirname "$0")" && pwd)
home=$(realpath "$HOME")
out=$home/.cache/lobby-test/without-launchers
lobby=${LOBBY:-$home/.local/bin/lobby}

# What uninstalling and clearing out each launcher leaves gone: its data and Wine setups, not
# the games it installed. Steam stays: Lobby needs it.
gone=(
    .local/share/lutris .config/lutris .cache/lutris .var/app/net.lutris.Lutris
    .config/heroic .config/heroic_gogdl .config/legendary .var/app/com.heroicgameslauncher.hgl Games/Heroic/Prefixes
    .config/faugus-launcher .var/app/io.github.Faugus.faugus-launcher Faugus
    .local/share/bottles .var/app/com.usebottles.bottles
    .local/share/flatpak/app/com.usebottles.bottles .local/share/flatpak/app/net.lutris.Lutris
    .local/share/flatpak/app/com.heroicgameslauncher.hgl .local/share/flatpak/app/io.github.Faugus.faugus-launcher
    ${ALSO_GONE:-}
)

view_args() {
    local gone_too=$1
    args=(--ro-bind / / --dev /dev --proc /proc --tmpfs "$home" --tmpfs "/run/user/$(id -u)" --tmpfs /tmp --perms 1777 --dir /tmp/.X11-unix)
    local path name
    for path in "$home"/.[!.]* "$home"/*; do
        [ -e "$path" ] || [ -L "$path" ] || continue
        name=${path#"$home"/}
        if [ "$gone_too" = yes ] && printf '%s\n' "${gone[@]}" | grep -qxF "$name"; then
            continue
        fi
        if [ -L "$path" ]; then
            args+=(--symlink "$(readlink "$path")" "$path")
        elif [ -d "$path" ] && bwrap --ro-bind / / --overlay-src "$path" --tmp-overlay "$path" true 2>/dev/null; then
            args+=(--overlay-src "$path" --tmp-overlay "$path")
        else
            args+=(--ro-bind "$path" "$path")
        fi
    done
    if [ "$gone_too" = yes ]; then
        args+=(--tmpfs "$home/.local/share/Trash")
        for path in /usr/bin/lutris /usr/bin/heroic /usr/bin/faugus-launcher; do
            [ -e "$path" ] && args+=(--ro-bind /dev/null "$path")
        done
        for path in /var/lib/flatpak/app/com.usebottles.bottles /var/lib/flatpak/app/net.lutris.Lutris /var/lib/flatpak/app/com.heroicgameslauncher.hgl /var/lib/flatpak/app/io.github.Faugus.faugus-launcher; do
            [ -e "$path" ] && args+=(--tmpfs "$path")
        done
    fi
    mkdir -p "$out"
    args+=(--bind "$out" "$out" --unshare-net --unshare-pid --die-with-parent --chdir "$home" --setenv HOME "$home" --unsetenv DBUS_SESSION_BUS_ADDRESS --unsetenv DISPLAY --unsetenv WAYLAND_DISPLAY)
}

# Lobby scans with the launchers there first, as it would have before they went. The rest of the
# removal happens inside: a layer can only hide a folder from the inside.
inside() {
    local gone_too=$1
    shift
    view_args "$gone_too"
    local remove=()
    if [ "$gone_too" = yes ]; then
        for path in "${gone[@]}"; do
            [[ $path == */* ]] && remove+=("$path")
        done
    fi
    bwrap "${args[@]}" --setenv LOBBY "$lobby" bash -c '"$LOBBY" scan >/dev/null 2>&1; n=$1; shift; for p in "${@:1:n}"; do rm -rf "${HOME:?}/$p"; done; shift "$n"; exec "$@"' _ "${#remove[@]}" "${remove[@]}" "$@"
}

case ${1:-check} in
    in)
        shift
        inside yes "$@"
        ;;
    check)
        rm -rf "$out"
        mkdir -p "$out/with" "$out/without"
        inside no python3 -I "$dir/without_launchers.py" collect "$lobby" "$out/with" || exit 1
        inside yes python3 -I "$dir/without_launchers.py" collect "$lobby" "$out/without" || exit 1
        python3 -I "$dir/without_launchers.py" compare "$out/with" "$out/without" | tee "$out/report.txt"
        ;;
    play)
        game=$2
        view_args "${GONE:-yes}"
        args+=(--dev-bind /dev /dev --setenv XDG_RUNTIME_DIR "/run/user/$(id -u)" --setenv SDL_AUDIODRIVER dummy --setenv LOBBY "$lobby" --setenv GAME "$game" --setenv PROTON "${PROTON:-}" --setenv OUT "$out")
        remove=()
        for path in "${gone[@]}"; do
            [[ $path == */* && ${GONE:-yes} == yes ]] && remove+=("$path")
        done
        timeout $((${WAIT:-45} + 60)) bwrap "${args[@]}" --setenv WAIT "${WAIT:-45}" dbus-run-session -- bash -c '
            for p in "$@"; do rm -rf "${HOME:?}/$p"; done
            if [ "${SCREEN:-kwin}" = gamescope ]; then
                # gamescope gives the game the focus itself (KWin moved it off a full-screen game
                # at the first key put in with XTEST, and the game minimised itself).
                setpriv --no-new-privs gamescope --backend headless -W 1280 -H 720 -w 1280 -h 720 -- sleep infinity >/dev/null 2>&1 &
                socket=gamescope-0
            else
                kwin_wayland --virtual --xwayland --no-lockscreen --width 1280 --height 720 --socket lobby-test >/dev/null 2>&1 &
                socket=lobby-test
            fi
            for _ in {1..20}; do [ -S "$XDG_RUNTIME_DIR/$socket" ] && break; sleep 0.5; done
            sleep 3
            x=$(ls /tmp/.X11-unix | head -1)
            [ -n "$x" ] || { echo "No X display: nothing here proves a picture."; exit 1; }
            export WAYLAND_DISPLAY=$socket DISPLAY=:${x#X}
            [ -n "$PROTON" ] && "$LOBBY" proton "$GAME" "$PROTON" --anyway 2>&1 | tail -1 | cut -c1-200
            "$LOBBY" how "$GAME" | sed -n 1,2p | cut -c1-200
            "$LOBBY" play "$GAME" 2>&1 | tail -5 | cut -c1-300 &
            # A check of its own while the game runs, on the same screen (DURING=<shell command>).
            [ -n "${DURING:-}" ] && bash -c "$DURING" &
            sleep "$WAIT"
            echo "--- running after $WAIT s"
            ps -eo args | grep -v -E "^(bash|bwrap|dbus|ps|grep|sleep|tail|cut|\[|kwin|Xwayland|ARGS|COMMAND|/usr/bin/(dbus|kwin|Xwayland)|.*kwin_|.*xdg-|.*wineserver|.*services.exe|.*winedevice|.*plugplay|.*rpcss|.*explorer.exe|.*svchost|.*tabtip|.*pressure-vessel|.*steam-runtime|.*reaper|.*xdg-dbus-proxy)" | cut -c1-200
            echo "--- its windows, and a picture of the screen in $OUT/$GAME.png"
            xwininfo -root -tree 2>/dev/null | grep -E "^ +0x" | grep -v -E "has no name|kwin|Xwayland" | head -8 | cut -c1-140
            window=$(xwininfo -root -tree 2>/dev/null | grep -E "^ +0x" | grep -v "has no name" | grep -E " [0-9]{3,5}x[0-9]{3,5}\+" | head -1 | awk "{print \$1}")
            [ -n "$window" ] && import -silent -window "$window" "$OUT/$GAME.png" 2>&1 | head -2
            [ -s "$OUT/$GAME.png" ] || echo "No picture was saved."
            echo "--- its log"
            tail -4 ~/.local/state/lobby/logs/"$GAME".log | cut -c1-200
            echo "--- saves and start record"
            "$LOBBY" how "$GAME" | grep -E "^(Saves|Last start)" | cut -c1-200
        ' _ "${remove[@]}"
        ;;
    *)
        sed -n 2,8p "$0"
        exit 2
        ;;
esac
