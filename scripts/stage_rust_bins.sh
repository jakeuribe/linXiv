#!/usr/bin/env bash
# Build the Rust CLI + MCP binaries and stage them into src-tauri/binaries/ with
# the Tauri target-triple suffix so `tauri build` bundles them (externalBin).
# Replaces the old PyInstaller specs + stage_sidecar.py (no Python sidecar now).
set -euo pipefail
cd "$(dirname "$0")/.."

triple="$(rustc -vV | sed -n 's/^host: //p')"
[ -n "$triple" ] || { echo "could not determine host target triple from rustc -vV" >&2; exit 1; }
exe=""
case "$triple" in *windows*) exe=".exe" ;; esac

( cd src-tauri && cargo build --release -p linxiv-cli -p linxiv-mcp )

dest="src-tauri/binaries"
mkdir -p "$dest"
cp "src-tauri/target/release/linxiv-cli$exe" "$dest/linxiv-$triple$exe"
cp "src-tauri/target/release/linxiv-mcp$exe" "$dest/linxiv-mcp-$triple$exe"
chmod +x "$dest/linxiv-$triple$exe" "$dest/linxiv-mcp-$triple$exe"
echo "staged: $dest/linxiv-$triple$exe"
echo "staged: $dest/linxiv-mcp-$triple$exe"
