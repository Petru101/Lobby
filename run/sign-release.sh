#!/bin/bash
# run/sign-release.sh <tag> [--no-upload]: signs a draft release's AppImage with the release key
# (~/.config/lobby-release/lobby.key, never on GitHub: LOBBY_RELEASE_KEY names a throwaway one to
# try this) and adds the signature to the release. Lobby refuses an update without it. Checks first
# that GitHub built the file from that tag and that the tag's Lobby knows this key.
set -e
tag=$1; upload=yes; [ "$2" = --no-upload ] && upload=
[ -n "$tag" ] || { echo "usage: run/sign-release.sh <tag> [--no-upload]" >&2; exit 1; }
repo=Petru101/Lobby
dir=$(cd "$(dirname "$0")/.." && pwd)
key=${LOBBY_RELEASE_KEY:-$HOME/.config/lobby-release/lobby.key}
public=${key%.key}.pub
[ -f "$key" ] && [ -f "$public" ] || { echo "no release key at $key" >&2; exit 1; }
# Apt's gh can be too old to have `gh attestation`.
checker=$HOME/.config/lobby-release/gh; [ -x "$checker" ] || checker=gh
"$checker" attestation verify --help >/dev/null 2>&1 || { echo "$checker can't check attestations: it needs gh 2.49 or newer" >&2; exit 1; }
work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
gh release download "$tag" --repo "$repo" --pattern Lobby-x86_64.AppImage --dir "$work"
"$checker" attestation verify "$work/Lobby-x86_64.AppImage" --repo "$repo" --signer-workflow "$repo/.github/workflows/release.yml" --source-ref "refs/tags/$tag" >/dev/null
echo "GitHub built it from $tag"
if [ -z "$LOBBY_RELEASE_KEY" ]; then
    git -C "$dir" fetch -q origin "refs/tags/$tag:refs/tags/$tag" 2>/dev/null || true
    git -C "$dir" show "$tag:src/appimage.rs" | grep -qF "\"$(sed -n 2p "$public")\"" || { echo "$tag's Lobby doesn't know this key: its updates would all be refused" >&2; exit 1; }
fi
minisign -S -s "$key" -m "$work/Lobby-x86_64.AppImage" -x "$work/Lobby-x86_64.AppImage.minisig" -t "Lobby ${tag#v}" >/dev/null
minisign -V -p "$public" -m "$work/Lobby-x86_64.AppImage" -x "$work/Lobby-x86_64.AppImage.minisig" >/dev/null
echo "signed for Lobby ${tag#v}"
if [ -n "$upload" ]; then
    gh release upload "$tag" "$work/Lobby-x86_64.AppImage.minisig" --repo "$repo"
    echo "added to the release: publish it with gh release edit $tag --repo $repo --draft=false"
fi
