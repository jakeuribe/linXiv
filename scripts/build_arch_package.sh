#!/usr/bin/env bash
# Build linXiv and package it as a native Arch Linux package.
set -euo pipefail
cd "$(dirname "$0")/.."

command -v makepkg >/dev/null || {
  echo "makepkg is required; install the base-devel package" >&2
  exit 1
}

npm run build:sidecar
npm run tauri build -- --no-bundle

(
  cd packaging/arch
  makepkg --force --clean --cleanbuild
)
