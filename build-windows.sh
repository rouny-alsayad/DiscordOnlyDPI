#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"

VERSION=$(awk -F '"' '/^version = "/ { print $2; exit }' Cargo.toml)
OUT="dist/DiscordOnlyDPI-v${VERSION}-windows-x64.exe"

cargo fmt --check
cargo check
cargo clippy -- -D warnings
cargo xwin build --release --target x86_64-pc-windows-msvc

mkdir -p dist
cp target/x86_64-pc-windows-msvc/release/discord-only-dpi.exe "$OUT"

echo "Built: $OUT"
sha256sum "$OUT"
