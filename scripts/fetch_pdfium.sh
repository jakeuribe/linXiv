#!/usr/bin/env bash
# Fetch the prebuilt pdfium native library (bblanchon) for the host target into
# src-tauri/vendor/pdfium/ (gitignored). Needed to build/test linxiv-core's PDF
# extraction. CI and fresh checkouts run this once; re-run on a new host/OS.
#
# PINNED to a specific release so the pdfium C ABI matches pdfium-render 0.8.x —
# "latest" can drift ahead of the bindings and segfault at runtime (uncatchable).
# The asset is sha256-verified before it is unpacked + dlopen'd in-process.
set -euo pipefail

PIN="chromium/7906"   # bblanchon release tag (libpdfium build 7906)
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="$ROOT/src-tauri/vendor/pdfium"

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64)   ASSET=pdfium-linux-x64.tgz ASSET_LIB_DIR=lib SHA=e07bc44c4e422c50eb01da742dc1ec59ad6780ce64ed91955533da8e9fe1a363 ;;
  Linux-aarch64)  ASSET=pdfium-linux-arm64.tgz ASSET_LIB_DIR=lib SHA= ;;
  Darwin-x86_64)  ASSET=pdfium-mac-x64.tgz   ASSET_LIB_DIR=lib SHA= ;;
  Darwin-arm64)   ASSET=pdfium-mac-arm64.tgz ASSET_LIB_DIR=lib SHA= ;;
  # Git Bash / MSYS2 report uname -s as MINGW64_NT-*, MSYS_NT-*, or CYGWIN_NT-*.
  # bblanchon packages the Windows DLL under bin/, not lib/, so it's staged into
  # $DEST/lib after extraction to match the layout the other platforms use (and
  # what tauri.conf.json's `vendor/pdfium/lib/` resource path expects).
  MINGW*-x86_64 | MSYS*-x86_64 | CYGWIN*-x86_64)
    ASSET=pdfium-win-x64.tgz ASSET_LIB_DIR=bin SHA=a60740ba8e9bddefa5a53113e25f62364995383d2eed032bcfc60f17209afe47 ;;
  *) echo "unsupported host $(uname -s)-$(uname -m); see github.com/bblanchon/pdfium-binaries/releases" >&2; exit 1 ;;
esac

mkdir -p "$DEST"
echo "[fetch_pdfium] downloading $PIN/$ASSET -> $DEST"
curl -fsSL -o "$DEST/$ASSET" "https://github.com/bblanchon/pdfium-binaries/releases/download/$PIN/$ASSET"

if [ -n "$SHA" ]; then
  echo "$SHA  $DEST/$ASSET" | sha256sum -c - \
    || { echo "[fetch_pdfium] CHECKSUM MISMATCH — refusing to unpack" >&2; rm -f "$DEST/$ASSET"; exit 1; }
else
  echo "[fetch_pdfium] WARNING: no pinned sha256 for $ASSET — verify and add one to this script" >&2
fi

tar xzf "$DEST/$ASSET" -C "$DEST"
rm -f "$DEST/$ASSET"

if [ "$ASSET_LIB_DIR" != "lib" ]; then
  mkdir -p "$DEST/lib"
  mv -f "$DEST/$ASSET_LIB_DIR"/* "$DEST/lib/"
fi

echo "[fetch_pdfium] done: $(find "$DEST" -name 'libpdfium*' -o -name 'pdfium.dll' | head)"
