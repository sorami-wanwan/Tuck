#!/usr/bin/env bash
set -euo pipefail

mkdir -p /workspace /cache/cargo-target /cache/bun

rsync -rlpt --delete \
  --exclude '.git' \
  --exclude 'node_modules' \
  --exclude 'dist' \
  --exclude 'src-tauri/target' \
  --exclude 'src-tauri/gen' \
  /src/ /workspace/

cd /workspace

if [ "$#" -gt 0 ]; then
  exec "$@"
fi

bun install --frozen-lockfile
bun run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
