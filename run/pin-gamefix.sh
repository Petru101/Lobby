#!/bin/bash
# Points Cargo.toml's gamefix at ../gaming's current commit, once that commit is on GitHub and
# nothing in ../gaming is left uncommitted. Run before a release: a release builds gamefix from
# GitHub at that commit, never from the working tree.
set -e
dir=$(cd "$(dirname "$0")/.." && pwd)
cd "$dir/../gaming"
[ -z "$(git status --porcelain)" ] || { echo "gamefix has uncommitted changes: commit and push them first" >&2; exit 1; }
git fetch -q origin
rev=$(git rev-parse HEAD)
git merge-base --is-ancestor "$rev" origin/main || { echo "gamefix's $rev isn't on GitHub's main: push it first" >&2; exit 1; }
sed -i -E "s|^(gamefix = \{ git = \"[^\"]+\", rev = \")[0-9a-f]+(\" \})|\1$rev\2|" "$dir/Cargo.toml"
grep -q "rev = \"$rev\"" "$dir/Cargo.toml" || { echo "Cargo.toml's gamefix line wasn't found" >&2; exit 1; }
echo "gamefix pinned at $rev"
