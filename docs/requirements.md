# Requirements

Everything you need installed before building linXiv from source. If you only want
to run the app, grab a prebuilt installer from the
[releases page](https://github.com/linxiv-dev/linXiv/releases/latest) instead — none
of this applies.

- [Toolchains](#toolchains)
- [System libraries](#system-libraries)
  - [Arch Linux](#arch-linux)
  - [Fedora / RHEL](#fedora--rhel)
  - [Debian / Ubuntu](#debian--ubuntu)
  - [macOS](#macos)
  - [Windows](#windows)
- [Verifying](#verifying)
- [Troubleshooting](#troubleshooting)

## Toolchains

| | Version | Why |
| --- | --- | --- |
| [Rust](https://rustup.rs/) | stable, 1.85+ | Backend, CLI, MCP server, and Tauri shell. The `linxiv-p2p` crate is `edition = "2024"`, which is the 1.85 floor. |
| [Node.js](https://nodejs.org/) | 20.16+ (22+ recommended) | Frontend and Tauri tooling. `pdfjs-dist` needs 20.16/22.3; `npm test` uses `--experimental-transform-types` and needs 22. |
| Git | any recent | The build uses submodules — see [Clone](../README.md#clone). |

Install Rust through `rustup` rather than a distro `rust` package. Tauri's tooling
expects a rustup-managed stable, and distro packages lag the edition-2024 floor.

Rust crates are fetched on the first `cargo`/`tauri` build; there is nothing to
install by hand. That includes the `tauri-plugin-texbrain` git dependency
(`github.com/linxiv-dev/tex-brain-linxiv-plugin`, pinned in `Cargo.lock`).

## System libraries

The Tauri shell links WebKit2GTK, which in turn links GTK 3 and GLib. Those
development headers must be present before `cargo` gets past the `gdk-sys` build
script — a missing one surfaces as a pkg-config failure rather than a compile error:

```
The system library `gdk-3.0` required by crate `gdk-sys` was not found.
The file `gdk-3.0.pc` needs to be installed and the PKG_CONFIG_PATH environment
variable must contain its parent directory.
```

Install the whole set in one go. Fixing them one error at a time just walks you up
the dependency chain: `gdk-sys` → `webkit2gtk-sys` → `libappindicator` → …

### Arch Linux

Install the native build dependencies from the official repositories:

```bash
sudo pacman -S --needed appmenu-gtk-module base-devel curl file git glib2 gtk3 \
  libappindicator librsvg openssl patchelf webkit2gtk-4.1 wget xdotool
```

After installing [Rust and Node.js](#toolchains), clone and build linXiv:

```bash
git clone --recurse-submodules https://github.com/linxiv-dev/linXiv.git
cd linXiv
npm install
npm run build:arch
```

`npm run build:arch` downloads PDFium, builds the app and its CLI/MCP sidecars,
and creates `packaging/arch/linxiv-<version>-1-x86_64.pkg.tar.zst`. Install the
result with pacman:

```bash
sudo pacman -U packaging/arch/linxiv-*.pkg.tar.zst
```

Unlike manually extracting a `.deb` or `.rpm`, this registers every installed
file with pacman so the package can be upgraded or removed normally.

### Fedora / RHEL

```bash
sudo dnf install gtk3-devel webkit2gtk4.1-devel glib2-devel \
  libappindicator-gtk3-devel librsvg2-devel libxdo-devel \
  openssl-devel patchelf gcc-c++ file
```

### Debian / Ubuntu

```bash
sudo apt install libgtk-3-dev libwebkit2gtk-4.1-dev libglib2.0-dev \
  libappindicator3-dev librsvg2-dev libxdo-dev \
  libssl-dev patchelf build-essential file
```

This mirrors what `.github/workflows/ci.yml` installs, so it is the set that is
continuously exercised.

### macOS

```bash
xcode-select --install
```

WebKit ships with the OS, so there is no GTK stack to install.

### Windows

Install the [Microsoft C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)
(the "Desktop development with C++" workload) and the
[WebView2 runtime](https://developer.microsoft.com/microsoft-edge/webview2/) —
WebView2 is already present on Windows 11 and up-to-date Windows 10.

Other distributions and edge cases are covered by the
[Tauri v2 prerequisites guide](https://tauri.app/start/prerequisites/), which is the
authoritative list.

## Verifying

```bash
rustc --version                  # 1.85.0 or newer
node --version                   # v20.16 or newer
pkg-config --modversion gtk+-3.0 webkit2gtk-4.1 glib-2.0    # Linux only
```

If `pkg-config` prints all three versions, the `gdk-sys` build script will get
through. From there, `npm run tauri dev` is the next step — see
[Setup](../README.md#setup).

## Troubleshooting

**pkg-config says a library is missing that you know you installed.**
Check `PKG_CONFIG_PATH`. Entries there are searched *before* the system directories
(`/usr/lib64/pkgconfig:/usr/share/pkgconfig`), so a leftover path pointing at a
hand-built prefix — a self-compiled GLib under `/opt`, say — shadows the packaged
`.pc` files with an older or incomplete copy. Unset it and retry:

```bash
env -u PKG_CONFIG_PATH cargo build
```

**`resource path "binaries/linxiv-<triple>" doesn't exist`, or the same for
`vendor/pdfium/lib`.**
These are gitignored paths that `tauri-build` validates at compile time, so they
must be staged before even `cargo check` will run. See
[Build notes](build.md#why-fetch_pdfiumsh-and-stage_rust_binssh-run-before-cargo-check).

**`failed to load manifest for dependency \`linxiv-p2p\``.**
`src-tauri/crates/p2p` is a git submodule *and* a Cargo workspace member, so an
un-initialised submodule breaks `cargo metadata` for the entire workspace:

```bash
git submodule update --init --recursive
```
