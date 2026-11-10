#!/usr/bin/env bash
# Builds the Windows exe and publishes it as a GitHub release.
# Installed copies of DiscordOnlyDPI update themselves from the latest release,
# which must contain both the exe and SHA256SUMS.txt.
set -euo pipefail
cd "$(dirname "$0")"

VERSION=$(awk -F '"' '/^version = "/ { print $2; exit }' Cargo.toml)
TAG="v${VERSION}"
NAME="DiscordOnlyDPI-${TAG}-windows-x64.exe"
NOTES="release-notes/${TAG}.md"

if [[ ! -f "$NOTES" ]]; then
    echo "Missing $NOTES" >&2
    exit 1
fi

if ! git diff --quiet || ! git diff --cached --quiet; then
    echo "Commit your changes before publishing." >&2
    exit 1
fi

if gh release view "$TAG" >/dev/null 2>&1; then
    echo "Release $TAG already exists on GitHub. Bump the version in Cargo.toml first." >&2
    exit 1
fi

bash build-windows.sh

if ! git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
    git tag -a "$TAG" -m "DiscordOnlyDPI $TAG"
fi
git push origin HEAD "refs/tags/$TAG"

gh release create "$TAG" \
    --title "DiscordOnlyDPI $TAG" \
    --notes-file "$NOTES" \
    "dist/$NAME" \
    dist/SHA256SUMS.txt

echo "Published $TAG. Installed copies will update on their next start."
