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

# Stage via a temp file + atomic rename. A running sidecar (e.g. the linxiv MCP
# server) holds the old binary, so an in-place `cp` fails with ETXTBSY ("Text
# file busy"); rename() swaps the directory entry without touching the busy inode.
stage_bin() {
  cp "$1" "$2.tmp"
  chmod +x "$2.tmp"
  mv -f "$2.tmp" "$2"
  echo "staged: $2"
}
stage_bin "src-tauri/target/release/linxiv-cli$exe" "$dest/linxiv-$triple$exe"
stage_bin "src-tauri/target/release/linxiv-mcp$exe" "$dest/linxiv-mcp-$triple$exe"
