# Architecture

linXiv is a Tauri v2 app. The frontend is React 18 + TypeScript (Vite); the backend is native Rust and runs **in-process** inside the app — the webview calls it through a single `api` Tauri command over IPC, and streams PDFs and graph assets over a custom `linxiv://` scheme. There is no HTTP server and no Python in the packaged app.

The Rust workspace lives under `src-tauri/` (which is also the Cargo workspace root):

```
linXiv/
├── src/                        # React + TypeScript frontend (Vite)
│   ├── api/                    # Typed client — calls the in-process backend via invoke("api")
│   ├── pages/ components/ …    # UI
├── public/graph/               # Force-directed graph viewer (Cytoscape + fCoSE + D3), loaded over linxiv://
├── src-tauri/                  # Tauri shell + Cargo workspace root
│   ├── src/                    # Tauri app: window, api-command router, integrations (install CLI/MCP)
│   │   └── bin/dev_server.rs   # linxiv-dev-server: dev-only HTTP shim over the Rust core (see Run in development)
│   ├── crates/
│   │   ├── core/               # linxiv-core: all library logic (sources, storage, formats, graph, service)
│   │   │   └── src/sources/    #   arXiv, OpenAlex, CrossRef, DOI resolution, PDF metadata, downloads
│   │   ├── cli/                # linxiv-cli → the `linxiv` binary (headless CLI)
│   │   ├── mcp/                # linxiv-mcp → the `linxiv-mcp` binary (MCP stdio server)
│   │   ├── migrate/            # one-off schema migration binary
│   │   ├── p2p/                # linxiv-p2p: vendored iroh transport, keyhive membership/roles, beelay E2EE sync
│   │   └── share/              # shared-project store — service layer over linxiv-p2p (publish/join/sync, encrypted key store)
│   ├── binaries/               # staged CLI + MCP sidecars (target-triple suffixed) for `tauri build`
│   ├── tauri.conf.json         # app config; bundles the linxiv + linxiv-mcp sidecars as externalBin
│   └── Cargo.toml              # workspace manifest
├── scripts/
│   ├── fetch_pdfium.sh         # downloads the native libpdfium used for PDF text extraction
│   └── stage_rust_bins.sh      # builds + stages the CLI/MCP sidecars into src-tauri/binaries/
└── assets/                     # logo, icons, GIFs
```

**Storage.** SQLite is compiled in via `rusqlite` (bundled, FTS5 for full-text search) — no system `libsqlite3` needed. The database is `papers.db` in the per-user app data directory.

**PDF extraction.** First-page text and metadata come from native `libpdfium` (`pdfium-render`). The shared library is fetched by `scripts/fetch_pdfium.sh` and bundled as a Tauri resource.
