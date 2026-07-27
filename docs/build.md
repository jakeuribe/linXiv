# Build notes

## Why `fetch_pdfium.sh` and `stage_rust_bins.sh` run before `cargo check`

**Both scripts are required before `cargo check`/`cargo build`/`tauri dev` will even compile — not just for a full `tauri build`.**

`src-tauri/tauri.conf.json` bundles `linxiv`/`linxiv-mcp` as Tauri sidecars (`bundle.externalBin`) and `vendor/pdfium/lib/` as a resource, and `tauri-build`'s build script validates all of these paths *at compile time*. They're gitignored, so on a fresh checkout `cargo check --workspace` fails first with `resource path "binaries/linxiv-<triple>" doesn't exist`, then (once the sidecars are staged) with `resource path "vendor/pdfium/lib" doesn't exist`.

`fetch_pdfium.sh` downloads libpdfium into `src-tauri/vendor/pdfium/`; `stage_rust_bins.sh` runs `cargo build --release -p linxiv-cli -p linxiv-mcp` and copies the binaries into `src-tauri/binaries/` with the host target-triple suffix (`npm run build:sidecar` runs both).

Re-run `stage_rust_bins.sh` whenever `linxiv-cli`/`linxiv-mcp` source changes and you need the sidecars to reflect it.
