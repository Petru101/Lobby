#!/bin/bash
# Builds and runs lobby on the host (it reads the host's drives and launchers): run/lobby.sh scan
set -o pipefail
dir=$(cd "$(dirname "$0")/.." && pwd)
"$dir/run/cargo.sh" build --release -q || exit 1
exec host-spawn "$dir/target/release/lobby" "$@"
