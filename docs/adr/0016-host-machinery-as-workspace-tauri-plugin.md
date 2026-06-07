# Host-side editor delivery implemented as a Tauri plugin in the sibling tex-brain-tauri repo (`tauri-plugin-texbrain`)

The machinery that downloads, caches, and serves the Editor plugin (ADR 0017) is implemented as a **Tauri plugin crate in the sibling tex-brain-tauri repo, consumed as a path dependency**, `tauri-plugin-texbrain`, rather than inlined in the app's `main.rs`. The work is exactly plugin-shaped — a custom URI-scheme handler that serves the cached editor + TeXLive (with the SwiftLaTeX `fileid`/`pkid` headers), commands invoked from the React frontend (`install`, `check_updates`, `uninstall`, `status`), a permissions manifest, and a guest-js TS API — so the official plugin template (`tauri plugin new`) fits cleanly and yields a first-class permissions boundary plus a tidy JS API.

It is a **path dependency on the sibling repo (not a workspace member of linxiv-texbrain, and not a published crate)**: this is single-app use, so we skip the versioning/publishing overhead. Tauri v2 plugins can register a URI scheme via `register_uri_scheme_protocol` (added for exactly this purpose).

**Terminology:** this "Tauri plugin" (a framework crate) is distinct from the "Editor plugin" (the downloadable product of ADR 0017). The Tauri plugin is *how the Host serves* the Editor plugin.

## Consequences

- The editor is served from a **custom URI scheme** (`texbrain://localhost`), a **separate origin** from the host SPA — see ADR 0015 for the addressing/origin consequences.
- The plugin also ports tex-brain's `texlive://` scheme handler (repointed from the bundle to the app-data cache) to serve TeXLive lazily out of the compressed cache.
- The plugin template scaffolds Android/iOS stubs we don't need (desktop-only); they are ignored/removed.
- **Confirmed** (spike `wl6398iw4`, verified against `tauri 2.11.2` source): runtime-downloaded content cannot be served same-origin under the app's `tauri://` origin, so the custom scheme is required — this is the established in-repo pattern (tex-brain serves TeXLive via a separate `texlive://` scheme).

## Extraction plan (decided 2026-06-06): generic core out at plugin #2

The concern is **file/navigability bloat** for contributors, not byte size: the
host repo should stay "app + thin plugin manager", with real functionality
delivered as downloaded artifacts (ADR 0017), never accreting in-tree.

Measured split of the crate today (~1,900 lines): roughly **80% is a generic
artifact-plugin manager** — `install.rs` (release fetch, streamed sha256,
semver/protocol gating), `cache.rs` (staging/active layout, atomic promote,
LRU + zip serving), `desktop.rs` (install/uninstall/status lifecycle) and their
tests — and **~20% is genuinely TeXbrain-specific**: `serve.rs` (the
`texbrain://`/`texlive://` scheme pair, SwiftLaTeX kpse candidate probing, SPA
fallback, editor CSP) plus a few constants (repo slug, manifest field names,
supported bridge protocols).

**Decision:** stay vendored as-is while there is exactly one plugin —
extracting now would mean maintaining a generic abstraction with a single
consumer and guessed seams. **The trigger is the second downloadable plugin**:
at that point, split the generic core into its own crate (working name
`tauri-plugin-artifact-cache`; in-tree first, own repo/crates.io only if shared
beyond linXiv), and reduce each per-plugin crate to a thin descriptor — feed
URL, scheme names, candidate-resolution rules, CSP. `serve.rs` is the model
for what a per-plugin adapter keeps. The current layering was kept
manager-vs-domain clean specifically so this cut stays cheap.
