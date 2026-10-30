#!/bin/bash
# Builds Lobby-x86_64.AppImage in target/appimage. Run on the oldest system it should start on
# (Ubuntu 24.04: its glibc and GTK 4.14 are what the AppImage needs at least), with Lobby's
# build packages and Rust installed (README). Bundles GTK, libadwaita and WebKitGTK; the system
# provides the rest (glibc, graphics drivers, bubblewrap).
set -e -o pipefail
dir=$(cd "$(dirname "$0")/.." && pwd)
out=$dir/target/appimage
tools=${XDG_CACHE_HOME:-$HOME/.cache}/lobby-appimage-tools
appdir=$out/AppDir
mkdir -p "$tools" "$out"

cargo build --release --manifest-path "$dir/Cargo.toml"

# The tools that pack it, once.
fetch() {
	[ -s "$tools/$1" ] || curl -fL --retry 3 -o "$tools/$1" "$2"
	chmod +x "$tools/$1"
}
fetch linuxdeploy-x86_64.AppImage https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage
fetch linuxdeploy-plugin-gtk.sh https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh
fetch appimagetool-x86_64.AppImage https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
export APPIMAGE_EXTRACT_AND_RUN=1 PATH=$tools:$PATH

rm -rf "$appdir"
mkdir -p "$appdir/usr/share/applications"
cat > "$appdir/usr/share/applications/io.github.Petru101.Lobby.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Lobby
GenericName=Game library
Comment=Every game on this computer in one place
Exec=lobby
Icon=io.github.Petru101.Lobby
Categories=Game;
Keywords=games;launcher;steam;heroic;lutris;
StartupWMClass=io.github.Petru101.Lobby
EOF

# WebKit's helper programs: started by path from its library, which names the system's folder
# for them. Copied into wk/, and that one name in the library changed to ./wk//// (slashes to
# the same length): relative to the window's working folder, which Lobby sets to the AppImage's
# (appimage.rs). WebKit's page sandbox makes the same path inside itself, as a new folder at its
# top (a name under its read-only system folders couldn't be made there).
webkit=/usr/lib/x86_64-linux-gnu/webkitgtk-6.0
cp -a "$webkit" "$appdir/wk"
# GLib's TLS (glib-networking), for WebKit's network process (its wrapper below): the bundled
# GLib would look for it where Ubuntu keeps it, not where other systems do.
mkdir -p "$appdir/usr/lib/gio/modules"
cp /usr/lib/x86_64-linux-gnu/gio/modules/libgiognutls.so "$appdir/usr/lib/gio/modules/"

DEPLOY_GTK_VERSION=4 linuxdeploy-x86_64.AppImage --appdir "$appdir" \
	--executable "$dir/target/release/lobby" \
	--desktop-file "$appdir/usr/share/applications/io.github.Petru101.Lobby.desktop" \
	--icon-file "$dir/data/io.github.Petru101.Lobby.svg" \
	--deploy-deps-only "$appdir/wk" \
	--deploy-deps-only "$appdir/usr/lib/gio/modules" \
	--plugin gtk

# Lobby never prints (GTK's print add-on would look for the system's CUPS).
rm -rf "$appdir/usr/lib/gtk-4.0/4.0.0/printbackends"

# The GTK plugin's start-up script forces the Adwaita theme (which spoils libadwaita's own look
# and dark mode) and X11; ours sets only where the bundled GTK finds its parts, and keeps the
# system's values for what Lobby starts (appimage.rs puts them back).
cat > "$appdir/apprun-hooks/linuxdeploy-plugin-gtk.sh" <<'EOF'
export APPDIR="${APPDIR:-"$(dirname "$(realpath "$0")")"}"
for v in XDG_DATA_DIRS GSETTINGS_SCHEMA_DIR GTK_PATH GTK_EXE_PREFIX GTK_DATA_PREFIX GDK_PIXBUF_MODULE_FILE GI_TYPELIB_PATH; do
	[ -n "${!v+x}" ] && export "LOBBY_HOST_$v=${!v}"
done
export XDG_DATA_DIRS="$APPDIR/usr/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
export GSETTINGS_SCHEMA_DIR="$APPDIR/usr/share/glib-2.0/schemas"
export GTK_PATH="$APPDIR/usr/lib/gtk-4.0"
export GTK_EXE_PREFIX="$APPDIR/usr"
export GTK_DATA_PREFIX="$APPDIR"
export GDK_PIXBUF_MODULE_FILE="$APPDIR/usr/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache"
export GI_TYPELIB_PATH="$APPDIR/usr/lib/girepository-1.0"
EOF

lib=$(find "$appdir/usr/lib" -name 'libwebkitgtk-6.0.so.*' -type f | head -1)
[ -n "$lib" ] || { echo "libwebkitgtk-6.0 wasn't bundled" >&2; exit 1; }
python3 - "$lib" "$webkit" <<'PY'
import sys
lib, old = sys.argv[1], sys.argv[2].encode()
new = b"./wk" + b"/" * (len(old) - 4)
assert len(new) == len(old)
data = open(lib, "rb").read()
n = data.count(old)
if n == 0:
    sys.exit("WebKit's library doesn't name " + sys.argv[2])
open(lib, "wb").write(data.replace(old, new))
print(f"pointed {n} paths in {lib.rsplit('/', 1)[1]} into the AppImage")
PY

# appimagetool wants a PNG beside the desktop file, and as .DirIcon (file managers' icon).
rm -f "$appdir/io.github.Petru101.Lobby.svg"
cp "$dir/data/io.github.Petru101.Lobby.png" "$appdir/io.github.Petru101.Lobby.png"
cp "$dir/data/io.github.Petru101.Lobby.png" "$appdir/.DirIcon"

# TLS's trusted certificates: GnuTLS has Ubuntu's file for them built in, which other systems
# keep elsewhere (Fedora: /etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem). It's changed to
# ./ca.crt (slashes to the same length), and WebKit's network process, the only one using GLib's
# TLS, starts through a wrapper that links this system's file as ca.crt in a folder of the
# user's own and starts there, with GLib pointed at the bundled TLS module.
gnutls=$(find "$appdir/usr/lib" -name 'libgnutls.so.*' -type f | head -1)
python3 - "$gnutls" <<'PY'
import sys
lib, old = sys.argv[1], b"/etc/ssl/certs/ca-certificates.crt"
new = b"./" + b"/" * (len(old) - len(b"./ca.crt")) + b"ca.crt"
assert len(new) == len(old)
data = open(lib, "rb").read()
if data.count(old) == 0:
    sys.exit("GnuTLS doesn't name " + old.decode())
open(lib, "wb").write(data.replace(old, new))
PY
mv "$appdir/wk/WebKitNetworkProcess" "$appdir/wk/WebKitNetworkProcess.real"
cat > "$appdir/wk/WebKitNetworkProcess" <<'EOF'
#!/bin/sh
here=$(cd "$(dirname "$0")" && pwd)
export GIO_EXTRA_MODULES="$here/../usr/lib/gio/modules"
dir="${XDG_RUNTIME_DIR:-/tmp}/lobby-tls"
mkdir -p -m 700 "$dir" 2>/dev/null
[ -O "$dir" ] || dir=$(mktemp -d)
for ca in /etc/ssl/certs/ca-certificates.crt /etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem /etc/pki/tls/certs/ca-bundle.crt /etc/ssl/ca-bundle.pem /etc/ssl/cert.pem; do
	[ -e "$ca" ] && { ln -sfn "$ca" "$dir/ca.crt"; break; }
done
cd "$dir" && exec "$here/WebKitNetworkProcess.real" "$@"
EOF
chmod +x "$appdir/wk/WebKitNetworkProcess"

version=$(grep -m1 '^version' "$dir/Cargo.toml" | cut -d'"' -f2)
# Update information: Gear Lever, AppImageUpdate and the like update it from GitHub's latest
# release, fetching only what changed through the .zsync file (attach both to each release).
# Lobby updates itself from the same releases (appimage.rs).
cd "$out"
ARCH=x86_64 VERSION=$version appimagetool-x86_64.AppImage --no-appstream \
	-u "gh-releases-zsync|Petru101|Lobby|latest|Lobby-x86_64.AppImage.zsync" "$appdir" Lobby-x86_64.AppImage
ls -la "$out"/Lobby-x86_64.AppImage*
