#!/usr/bin/env bash
# One-command release: bump the workspace version, commit, tag, and push.
# cargo-dist's release workflow then runs on the pushed tag.
#
# Usage: scripts/release.sh 1.2.0   (or v1.2.0)
set -euo pipefail

VERSION="${1:?usage: scripts/release.sh <version> (e.g. 1.2.0)}"
VERSION="${VERSION#v}"

if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]]; then
  echo "error: '$VERSION' is not a valid semver version" >&2
  exit 1
fi

cd "$(git rev-parse --show-toplevel)"

if [[ -n "$(git status --porcelain)" ]]; then
  echo "error: working tree is not clean; commit or stash first" >&2
  exit 1
fi

if git rev-parse "v$VERSION" >/dev/null 2>&1; then
  echo "error: tag v$VERSION already exists" >&2
  exit 1
fi

# Update [workspace.package] version (first top-level `version =` line only)
perl -i -pe 's/^version = ".*"$/version = "'"$VERSION"'"/ && ($done = 1) unless $done' Cargo.toml

# Refresh Cargo.lock with the new workspace member versions
cargo check --quiet

git add Cargo.toml Cargo.lock
git commit -m "v$VERSION"
git tag "v$VERSION"
git push origin HEAD "v$VERSION"

echo "Released v$VERSION — watch https://github.com/fuwasegu/specgraphen/actions"
