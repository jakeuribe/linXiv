# linXiv

<p align="center">
  <a href="https://github.com/linxiv-dev/linXiv/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/linxiv-dev/linXiv/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://github.com/linxiv-dev/linXiv/releases"><img alt="Release" src="https://img.shields.io/github/v/release/linxiv-dev/linXiv?include_prereleases"></a>
  <a href="LICENSE"><img alt="License: GPL v3" src="https://img.shields.io/badge/License-GPLv3-blue.svg"></a>
  <a href="https://tauri.app"><img alt="Tauri" src="https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=fff"></a>
</p>

<p align="center">
  <a href="https://discord.gg/RfbuxuGt5"><img src="https://dcbadge.limes.pink/api/server/RfbuxuGt5" alt="" /></a>
</p>
<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="assets/logo-light.svg">
    <img alt="Project Logo" src="assets/svg_logo.svg" width="180">
  </picture>
</p>

A local-first desktop application for discovering, managing, and visualizing academic papers from arXiv and other sources. It bundles a native Rust backend (bundled SQLite storage, arXiv/OpenAlex/CrossRef sources, PDF text extraction, BibTeX/Obsidian export) with a React + TypeScript frontend and an interactive paper–author graph, all wrapped in a Tauri v2 desktop shell.

Upload your PDFs, create projects, manage notes, tags, and annotations to organize your library; all locally. linXiv aims to be a one-stop shop for researchers managing their literature, with the near-term goal of extending to research groups who want to share knowledge without going to the web.

> **Development status:** Pre-1.0 (current version `0.3.3`). The database schema is still evolving, but migration structure is in-place.

> **Licensing:** linXiv is GPLv3. The vendored [`linxiv-p2p`](https://github.com/linxiv-dev/linxiv-p2p) submodule (`src-tauri/crates/p2p`) is licensed separately under Apache-2.0.

<p align="center">
  <a href="https://youtu.be/c4vQuXjFv34">
    <img src="https://img.youtube.com/vi/c4vQuXjFv34/maxresdefault.jpg" width="800" alt="Watch the linXiv demo">
  </a>
  <br>
  <em><a href="https://youtu.be/c4vQuXjFv34">▶ Watch the full demo (3:16)</a></em>
</p>

## Install

Prebuilt installers for Linux, macOS, and Windows are on the [releases page](https://github.com/linxiv-dev/linXiv/releases/latest):

| Platform | Download |
| --- | --- |
| Linux | `.deb`, `.rpm`, or `.AppImage` |
| macOS (Apple silicon) | `.dmg` (arm64) |
| macOS (Intel) | `.dmg` (x86_64) |
| Windows | `.exe` (NSIS) or `.msi` |

The macOS and Windows builds are unsigned.

**macOS:** On macOS 15 (Sequoia) and later, Apple removed the right-click → **Open** Gatekeeper bypass. The first launch will say **"linXiv is damaged and can't be opened."** This is not a broken download; it's Gatekeeper blocking an unsigned app. To open it:
- Try launching the app once (it will fail with the "damaged" message), then go to **System Settings → Privacy & Security** and click **Open Anyway** next to the linXiv block notice, or
- Run `xattr -cr /Applications/linXiv.app` in a terminal to strip the quarantine attribute, then launch normally.

On macOS 14 and earlier, right-click the app → **Open** still works to get past Gatekeeper the first time.

**Windows:** click **More info** → **Run anyway** on the SmartScreen prompt.

### Installing via pip install

You can install linxiv via pip! It ships with a small API to programatically control your database in python and can be installedi as such

```sh
pip install linxiv          # everything below
pip install "linxiv[app]"   # same, extras are opt-in markers only
pip install "linxiv[cli]"
```

To build from source instead, start at [Clone](#clone).

## Clone

This repo has git submodules (`docs/adr`, `src-tauri/crates/p2p`). The npm scripts that touch Rust (`tauri`, `dev:api`, `build:all`) init them for you, so a plain `git clone` is fine if you build through npm. Cloning with submodules up front is still the fastest path, and is required if you run `cargo` directly inside `src-tauri/`:

```bash
git clone --recurse-submodules https://github.com/linxiv-dev/linXiv.git
# already cloned without --recurse-submodules?
git submodule update --init --recursive
```

## Table of Contents

- [Install](#install)
- [Clone](#clone)
- [Features](#features)
- [Architecture](#architecture)
- [Setup](#setup)
  - [Prerequisites](#prerequisites)
  - [Install dependencies](#install-dependencies)
  - [Run in development](#run-in-development)
- [Building the desktop app](#building-the-desktop-app)
- [CLI](#cli)
- [MCP server](#mcp-server)
- [Headless server](#headless-server)
- [Graph visualization](#graph-visualization)
- [Data location](#data-location)
- [Acknowledgements](#acknowledgements)

## Features

- **Paper search & fetch** — Search arXiv, OpenAlex, or CrossRef by keyword; fetch by ID; resolve by DOI (arXiv → Semantic Scholar → CrossRef fallback). Results are saved to a local SQLite database with per-paper version tracking.
- **Projects** — Organize papers into projects; scope notes and highlight annotations to a paper within a project; archive, restore, and trash with soft-delete.
- **Notes & PDF annotations** — Attach freeform notes and PDF highlight annotations to papers, optionally scoped to a project.
- **Tags** — Tag papers and projects; list and manage the full tag set.
- **PDF management** — Download PDFs, import local PDFs (with first-page text and metadata extraction via native PDFium), and track total storage usage.
- **Full-text search** — Pull an arXiv paper's TeX source into a local SQLite FTS5 index, from the paper page, the CLI (`linxiv paper fetch-source` / `index-sources`), or the `fetch_full_text` MCP tool; library search then matches the paper's body, not just its metadata.
- **Import / export** — Import and export projects as `.lxproj` archives, import BibTeX (`.bib`), and export projects to BibTeX or Obsidian-flavored Markdown.
- **Interactive graph** — Force-directed network of papers, authors and tags (Cytoscape rendering a d3-force layout), with real-time force controls and filter panels.
- **TeX rendering** — MathJax renders LaTeX math in titles and abstracts, bundled locally for full offline use.
- **CLI & MCP server** — A headless `linxiv` CLI and an `linxiv-mcp` MCP server expose the same library over the terminal and to LLM clients such as Claude.
- **Peer-to-peer project sharing** — Share a project over [iroh](https://www.iroh.computer/) (QUIC + node tickets, no relay server to run) with end-to-end encrypted sync via keyhive + beelay CRDTs; you're the Hoster or a Reader of a share, and a Hoster can invite members as Editor or Viewer; join with a pasted ticket, mirror shared projects into your local library, and sync on your own schedule.

## Architecture

linXiv is a Tauri v2 app. The frontend is React 18 + TypeScript (Vite); the backend is native Rust and runs **in-process** inside the app: the webview calls it through a single `api` Tauri command over IPC, and streams PDF bytes over a custom `linxiv://` scheme. SQLite (bundled, FTS5) and PDF extraction (native `libpdfium`) are compiled in; see [docs/architecture.md](docs/architecture.md) for the full workspace layout.

## Setup

### Prerequisites

- [Rust toolchain](https://rustup.rs/) (stable, 1.85+) — builds the backend, CLI, MCP server, and Tauri shell
- [Node.js](https://nodejs.org/) 20.16+ (22+ recommended) — frontend / Tauri tooling
- System libraries — GTK 3, WebKit2GTK 4.1, and GLib on Linux; Xcode Command Line Tools on macOS; Microsoft C++ Build Tools on Windows

**See [docs/requirements.md](docs/requirements.md) for the copy-pasteable install commands per OS**, exact version reasoning, and what to do when `pkg-config` reports `gdk-3.0` missing.

### Install dependencies

```bash
npm install                       # frontend dependencies
bash scripts/fetch_pdfium.sh      # native libpdfium (PDF import/extraction; also a bundled Tauri resource, see below)
bash scripts/stage_rust_bins.sh   # builds + stages the linxiv/linxiv-mcp sidecars (see below)
```

Rust crates are fetched automatically on first `cargo`/`tauri` build.

> **Both scripts above are required before `cargo check`/`cargo build`/`tauri dev` will even compile, not just for a full `tauri build`** (they stage gitignored paths that `tauri-build` validates at compile time). See [docs/build.md](docs/build.md) if you hit a `resource path ... doesn't exist` error.

### Run in development

Native desktop window (recommended: runs the in-process Rust backend, hot-reloads the frontend):

```bash
npm run tauri dev
```

Browser-only dev loop (no native window; uses a dev-only HTTP shim that serves the backend over `/api`):

```bash
# terminal 1 — dev backend (linxiv-dev-server, HTTP shim over the Rust core)
npm run dev:api

# terminal 2 — Vite dev server on :5180 (proxies /api to the shim)
npm run dev
```

## Building the desktop app

The `linxiv` (CLI) and `linxiv-mcp` (MCP server) binaries ship inside the app as Tauri sidecars.

Fresh checkout, "just give me an installer":

```bash
npm run build:all       # = build:sidecar + tauri build
```

`build:all` is only a convenience wrapper. The steps under it are
independently re-runnable, and most of them are one-time setup — a repeat
build usually only needs `npm run tauri build`:

| Command | What it does | When you need it |
|---|---|---|
| `bash scripts/fetch_pdfium.sh` | downloads the pinned native libpdfium into `src-tauri/vendor/pdfium/` | once per machine/OS; again only when the pin in the script changes |
| `bash scripts/stage_rust_bins.sh` | builds `linxiv-cli` + `linxiv-mcp` and stages them into `src-tauri/binaries/` | after changing the CLI/MCP crates — otherwise the previously staged sidecars ship as-is |
| `npm run build:sidecar` | both of the above | fresh checkout / new host |
| `npm run tauri build` | builds and bundles the app | always — this is the actual build |
| `npm run build:arch` | builds an Arch Linux pacman package | only when packaging for Arch |

The installer/bundle is written to `src-tauri/target/release/bundle/`.

After installing the app, open **Settings** to:
- **Install CLI** — symlinks the bundled `linxiv` binary to `~/.local/bin/linxiv` (Linux/macOS) or writes a PATH shim on Windows.
- **Integrations** — register the bundled MCP server with a detected client (Claude Desktop, Claude Code, and others) by writing its config file.

## CLI

The `linxiv` binary is a headless interface to the same library the app uses. In a checkout you can run it without installing:

```bash
# from src-tauri/
cargo run -p linxiv-cli -- --help
```

Installed (via the app's **Install CLI**, or a staged/bundled build), invoke it directly as `linxiv`. All commands print JSON to stdout; pass `--help` to any command or subcommand for full options.

```bash
linxiv --version
linxiv search "attention is all you need" --max 5
linxiv fetch 2204.12985
linxiv paper get 2204.12985
```

Covers papers, tags, projects, notes, PDF annotations, PDFs, DOI resolution, authors, BibTeX import, trash, and library maintenance; see [docs/cli_ref/](docs/cli_ref/) for the full command reference.

## MCP server

`linxiv-mcp` is a stdio MCP server exposing ~60 tools (search, fetch, papers, projects, tags, notes, annotations, PDFs, trash, authors, import/export, settings, stats) so an MCP client like Claude can drive your library directly.

The simplest path is to install the desktop app and use **Settings → Integrations**, which registers the bundled server with a detected client.

To register manually with the Claude Code CLI, point it at the built or bundled binary:

```bash
claude mcp add linxiv -- /path/to/linxiv-mcp
```

Or add it to a client's MCP config (e.g. `claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "linxiv": {
      "command": "/path/to/linxiv-mcp"
    }
  }
}
```

In a checkout you can run it straight from source with `cargo run -p linxiv-mcp` (from `src-tauri/`).

<img src="assets/claude_demo.gif" width="800" />

## Headless server

`linxiv-headless` runs the full backend — the complete `/api/*` surface,
the iroh share peer, and background sync — with no window, for a
self-hosted or containerized always-on node. Run it from source
(`cargo run -p linxiv-server --bin linxiv-headless` from `src-tauri/`) or
build the repo's `Dockerfile`; a bearer token gates every request when it
binds beyond loopback, and `GET /admin` serves a small management page.
Setup steps, a ready-made compose file, the environment reference, and
relay configuration: [docs/headless](docs/headless/README.md).

## Graph visualization

Papers and authors make up a force-directed network: papers link to their authors and tags, laid out by a d3-force simulation and drawn with Cytoscape. The control panel gives you real-time sliders to steer how the nodes and links work together, plus filters over categories, dates, tags and projects. Everything — the graph libraries, MathJax and the UI font — is bundled locally, so the graph works offline like the rest of the app.

## Data location

The database (`papers.db`), managed PDFs, and the Obsidian vault live in the per-user app data directory for `com.linxiv.app` (e.g. `~/.local/share/com.linxiv.app` on Linux, `~/Library/Application Support/com.linxiv.app` on macOS). Set the `LINXIV_DATA_DIR` environment variable to override the location; the app, CLI, and MCP server all honor it, so they share one library.

## Acknowledgements

Thank you to arXiv for use of its open access interoperability!

linXiv owes a debt to [Qiqqa](https://github.com/jimmejardine/qiqqa-open-source), the open-source research management tool originally created by Jimme Jardine.

PDF text and metadata extraction is currently powered by [PDFium](https://pdfium.googlesource.com/pdfium/) (Google's PDF rendering library) via the [`pdfium-render`](https://github.com/ajrcarey/pdfium-render) Rust bindings.
