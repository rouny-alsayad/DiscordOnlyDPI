#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
cargo fmt --check
cargo check
cargo clippy -- -D warnings
cargo xwin build --release --target x86_64-pc-windows-msvc
mkdir -p dist
cp target/x86_64-pc-windows-msvc/release/discord-only-dpi.exe dist/DiscordOnlyDPI-v0.1-windows-x64.exe
sha256sum dist/DiscordOnlyDPI-v0.1-windows-x64.exe
