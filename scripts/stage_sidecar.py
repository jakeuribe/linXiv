#!/usr/bin/env python3
"""
Copy all PyInstaller --onefile outputs to src-tauri/binaries/ with the
correct Tauri target-triple suffix so `npm run tauri build` can bundle them.

Binaries staged:
  dist/linxiv-api  -> src-tauri/binaries/linxiv-api-{triple}
  dist/linxiv-cli  -> src-tauri/binaries/linxiv-{triple}
  dist/linxiv-mcp  -> src-tauri/binaries/linxiv-mcp-{triple}
"""
import platform
import shutil
from pathlib import Path

TRIPLES = {
    ("linux",   "x86_64"):  "x86_64-unknown-linux-gnu",
    ("linux",   "aarch64"): "aarch64-unknown-linux-gnu",
    ("darwin",  "x86_64"):  "x86_64-apple-darwin",
    ("darwin",  "arm64"):   "aarch64-apple-darwin",
    ("windows", "amd64"):   "x86_64-pc-windows-msvc",
    ("windows", "x86_64"):  "x86_64-pc-windows-msvc",
}

system = platform.system().lower()
machine = platform.machine().lower()
triple = TRIPLES.get((system, machine))
if not triple:
    raise SystemExit(f"Unknown platform: {system}/{machine} -- add it to TRIPLES in this script")

ext = ".exe" if system == "windows" else ""
dest_dir = Path("src-tauri") / "binaries"
dest_dir.mkdir(parents=True, exist_ok=True)

for name, dest_name in [("linxiv-api", "linxiv-api"), ("linxiv-cli", "linxiv"), ("linxiv-mcp", "linxiv-mcp")]:
    src = Path("dist") / f"{name}{ext}"
    if not src.exists():
        print(f"  skip   {src}  (not built yet)")
        continue
    dest = dest_dir / f"{dest_name}-{triple}{ext}"
    shutil.copy2(src, dest)
    dest.chmod(dest.stat().st_mode | 0o111)
    print(f"  staged {dest}")
