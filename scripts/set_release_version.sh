#!/usr/bin/env bash
# Keep every shipped package's version aligned with the release tag.
set -euo pipefail
cd "$(dirname "$0")/.."

ver="${1#v}"
if [[ ! "$ver" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]]; then
  echo "invalid release version: $1" >&2
  exit 1
fi

node -e '
  const fs = require("node:fs");
  const path = "src-tauri/tauri.conf.json";
  const config = JSON.parse(fs.readFileSync(path, "utf8"));
  config.version = process.argv[1];
  fs.writeFileSync(path, `${JSON.stringify(config, null, 2)}\n`);
' "$ver"

npm version "$ver" --no-git-tag-version --allow-same-version

# linxiv-p2p is a separate vendored submodule; its version is independent.
for file in src-tauri/Cargo.toml src-tauri/crates/*/Cargo.toml; do
  [[ "$file" == "src-tauri/crates/p2p/Cargo.toml" ]] && continue
  sed -i.bak "s/^version = \".*\"/version = \"$ver\"/" "$file"
  rm -f "$file.bak"
done

# PKGBUILD reserves hyphens as separators between pkgver, pkgrel and arch.
arch_ver="${ver//-/_}"
sed -i.bak "s/^pkgver=.*/pkgver=$arch_ver/" packaging/arch/PKGBUILD
rm -f packaging/arch/PKGBUILD.bak
