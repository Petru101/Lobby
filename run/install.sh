#!/bin/bash
# Builds lobby and puts it in the app menu: ~/.local/bin/lobby and an entry in
# ~/.local/share/applications. Run again after changes to update it.
set -e -o pipefail
dir=$(cd "$(dirname "$0")/.." && pwd)
"$dir/run/cargo.sh" build --release -q
mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications"
# Through a temporary name: a running lobby keeps its old copy.
install -m 755 "$dir/target/release/lobby" "$HOME/.local/bin/lobby.new"
mv "$HOME/.local/bin/lobby.new" "$HOME/.local/bin/lobby"
cat > "$HOME/.local/share/applications/io.github.Petru101.Lobby.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Lobby
GenericName=Game library
Comment=Every game on this computer in one place
Exec=$HOME/.local/bin/lobby
Icon=applications-games
Categories=Game;
Keywords=games;launcher;steam;heroic;lutris;
StartupWMClass=io.github.Petru101.Lobby
EOF
host-spawn update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
echo "Lobby is in the app menu (Games)."
