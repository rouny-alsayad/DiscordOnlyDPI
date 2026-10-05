#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"

VERSION=$(awk -F '"' '/^version = "/ { print $2; exit }' Cargo.toml)
NAME="DiscordOnlyDPI-v${VERSION}-windows-x64.exe"
OUT="dist/${NAME}"

cargo fmt --check
cargo check
cargo clippy -- -D warnings
cargo xwin build --release --target x86_64-pc-windows-msvc

mkdir -p dist
cp target/x86_64-pc-windows-msvc/release/discord-only-dpi.exe "$OUT"

HASH=$(sha256sum "$OUT" | awk '{print $1}')
printf '%s  %s\n' "$HASH" "$NAME" > dist/SHA256SUMS.txt

echo "Built: $OUT"
echo "SHA-256: $HASH"
echo "Checksums: dist/SHA256SUMS.txt"
