#!/bin/bash
# Runs cargo from the GNOME SDK (rust-stable) on the host: run/cargo.sh build
# gamefix comes from GitHub; a checkout beside this one (../gaming) is built in instead, through
# .cargo/config.toml (written here, not committed). Crates are cached in ~/.cargo.
set -o pipefail
dir=$(cd "$(dirname "$0")/.." && pwd)
gamefix=$(cd "$dir/../gaming" 2>/dev/null && pwd)
if [ -n "$gamefix" ]; then
    mkdir -p "$dir/.cargo"
    printf '[patch."https://github.com/Petru101/gamefix"]\ngamefix = { path = "%s" }\n' "$gamefix" > "$dir/.cargo/config.toml"
else
    rm -f "$dir/.cargo/config.toml"
fi
host-spawn flatpak run --share=network --filesystem="$dir" ${gamefix:+--filesystem="$gamefix:ro"} --filesystem="$HOME/.cargo:create" \
    --command=sh org.gnome.Sdk//50 -c \
    'cd "$0" && PATH=/usr/lib/sdk/rust-stable/bin:$PATH exec cargo "$@"' \
    "$dir" "$@" </dev/null | tr -d '\r'
