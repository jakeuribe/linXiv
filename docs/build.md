# Build notes

## Why `fetch_pdfium.sh` and `stage_rust_bins.sh` run before `cargo check`

**Both scripts are required before `cargo check`/`cargo build`/`tauri dev` will even compile — not just for a full `tauri build`.**

`src-tauri/tauri.conf.json` bundles `linxiv`/`linxiv-mcp` as Tauri sidecars (`bundle.externalBin`) and `vendor/pdfium/lib/` as a resource, and `tauri-build`'s build script validates all of these paths *at compile time*. They're gitignored, so on a fresh checkout `cargo check --workspace` fails first with `resource path "binaries/linxiv-<triple>" doesn't exist`, then (once the sidecars are staged) with `resource path "vendor/pdfium/lib" doesn't exist`.

`fetch_pdfium.sh` downloads libpdfium into `src-tauri/vendor/pdfium/`; `stage_rust_bins.sh` runs `cargo build --release -p linxiv-cli -p linxiv-mcp` and copies the binaries into `src-tauri/binaries/` with the host target-triple suffix (`npm run build:sidecar` runs both).

Re-run `stage_rust_bins.sh` whenever `linxiv-cli`/`linxiv-mcp` source changes and you need the sidecars to reflect it.

## Never release a stable tag whose core matches an already-shipped prerelease

**After `v0.3.0-beta`, the next release is `v0.3.1` — not `v0.3.0`.**

`release.yml` writes the tag verbatim into `tauri.conf.json`, so `v0.3.0-beta`
ships as RPM `Version: 0.3.0-beta`. RPM has no notion of a semver prerelease —
its only prerelease marker is `~` — so a trailing alphabetic segment ranks
*above* the bare core: `rpm.vercmp("0.3.0-beta", "0.3.0") == 1`. The in-app
updater compares by semver (`compareVersions` in `src/api/updates.ts`) and
correctly sees `0.3.0` as newer, downloads the rpm, and then `rpm -U` refuses
it as a downgrade — the update button fails forever on every beta install.
Bumping the patch instead keeps both orderings agreeing.

The tags themselves are fine: `v0.3.0-beta` is valid semver, and marking the
GitHub release "pre-release" keeps it out of `releases/latest`, which is what
both the updater and `apply_linux_package_update` query. Only the version
*sequence* matters.

Anyone stuck on a beta build gets out with
`sudo rpm -U --oldpackage linXiv-<ver>.x86_64.rpm`.
