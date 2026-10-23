#!/bin/bash
# Runs cargo from the GNOME SDK (rust-stable) on the host: run/cargo.sh build
# gamefix (../gaming) is a path dependency; crates are cached in ~/.cargo.
set -o pipefail
dir=$(cd "$(dirname "$0")/.." && pwd)
gamefix=$(cd "$dir/../gaming" && pwd)
host-spawn flatpak run --share=network --filesystem="$dir" --filesystem="$gamefix:ro" --filesystem="$HOME/.cargo:create" \
    --command=sh org.gnome.Sdk//50 -c \
    'cd "$0" && PATH=/usr/lib/sdk/rust-stable/bin:$PATH exec cargo "$@"' \
    "$dir" "$@" </dev/null | tr -d '\r'
