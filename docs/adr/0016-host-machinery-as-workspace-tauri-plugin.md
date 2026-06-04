# Host-side editor delivery implemented as a workspace-local Tauri plugin (`tauri-plugin-texbrain`)

The machinery that downloads, caches, and serves the Editor plugin (ADR 0014) is implemented as a **workspace-local Tauri plugin crate**, `tauri-plugin-texbrain`, rather than inlined in the app's `main.rs`. The work is exactly plugin-shaped — a custom URI-scheme handler that serves the cached editor + TeXLive (with the SwiftLaTeX `fileid`/`pkid` headers), commands invoked from the React frontend (`install`, `check_updates`, `uninstall`, `status`), a permissions manifest, and a guest-js TS API — so the official plugin template (`tauri plugin new`) fits cleanly and yields a first-class permissions boundary plus a tidy JS API.

It is a **path-dependency workspace member, not a published crate**: this is single-app use, so we skip the versioning/publishing overhead. Tauri v2 plugins can register a URI scheme via `register_uri_scheme_protocol` (added for exactly this purpose).

**Terminology:** this "Tauri plugin" (a framework crate) is distinct from the "Editor plugin" (the downloadable product of ADR 0014). The Tauri plugin is *how the Host serves* the Editor plugin.

## Consequences

- The editor is served from a **custom URI scheme** (`texbrain://localhost`), a **separate origin** from the host SPA — see ADR 0015 for the addressing/origin consequences.
- The plugin also ports tex-brain's `texlive://` scheme handler (repointed from the bundle to the app-data cache) to serve TeXLive lazily out of the compressed cache.
- The plugin template scaffolds Android/iOS stubs we don't need (desktop-only); they are ignored/removed.
- **Confirmed** (spike `wl6398iw4`, verified against `tauri 2.11.2` source): runtime-downloaded content cannot be served same-origin under the app's `tauri://` origin, so the custom scheme is required — this is the established in-repo pattern (tex-brain serves TeXLive via a separate `texlive://` scheme).
