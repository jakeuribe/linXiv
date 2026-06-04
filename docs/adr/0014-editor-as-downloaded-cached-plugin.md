# Editor delivered as an online-downloaded, locally-cached plugin

---
Status: proposed (decision in progress — hosting, versioning, integrity, and TeXLive sourcing still being resolved)
---

The embedded LaTeX editor (tex-brain) is **not** bundled into the linXiv installer. Bundling the editor build plus its ~78MB TeXLive cache would bloat the installer and collide with tex-brain's size-reduction roadmap. Instead, **when the user explicitly chooses to install the plugin** (not automatically on first editor open) the Host downloads it from the internet, caches it in the app-data dir, and the Host Rust shell serves it same-origin at `/texbrain/*` (the iframe's `EDITOR_SRC`). This supersedes the "bundle TeXbrain static as a Tauri resource" mechanism in PORT_PLAN.md Milestone 2.

The plugin is **two independently-versioned, separately-cached artifacts** (see CONTEXT.md): the **Editor build** (SvelteKit SPA + SwiftLaTeX worker; a few MB; updates often) and the **TeXLive cache** (~78MB uncompressed; rarely changes). They are split because they differ ~20× in size and change at very different cadences — a monolithic bundle would force re-downloading TeXLive on every editor bugfix.

The TeXLive cache is **downloaded eagerly and in full at install time** (so the editor is fully offline afterward — no per-compile network), but **stored compressed at rest**; the serving scheme **decompresses individual files on demand and evicts them when idle**, keeping the disk/memory footprint well below the uncompressed 78MB. This requires an archive format with **random per-entry access by name** (ZIP central-directory seek, or seekable-zstd) — *not* a plain `.tar.gz` — which constrains what tex-brain's release tooling must emit.

**Distribution: GitHub Releases on `jakeuribe/tex-brain-tauri`.** A new CI workflow on that repo (it has none today) builds the editor (served at its origin root — no `base` pinning, see ADR 0015), produces the two compressed artifacts (editor build + TeXLive cache) plus a small `manifest.json` (plugin/bridge-protocol version, per-artifact URL + sha256 + uncompressed size), and uploads all three as release assets. linXiv reads the manifest to discover what to download and to verify it. The editor's outputs stay versioned in the editor's own repo; linXiv only consumes them.

In **development the plugin is not downloaded**: the editor is served from its own running dev server (via the Host's dev proxy — see ADR 0015); only packaged/production builds download + cache + serve from app-data.

## Versioning & updates

Version selection is **compatibility-gated on the bridge protocol**: the Editor's `texbrain:ready` handshake already carries a `protocol` integer, the `manifest.json` declares each release's protocol, and the Host installs/offers only the **newest release whose protocol the Host supports** — refusing an incompatible editor rather than letting the embed silently break. Updates are **user-driven**: the plugin downloads at the explicit install action, and afterward update-checking offers the newest compatible release. There is **no silent background auto-update** (it could swap the editor mid-session).

The plugin's update check is **not a plugin-specific button** — it is unified into linXiv's planned **global "Check for updates"** surface, which does not exist yet. Building the plugin's update check therefore seeds that global surface (which will later also cover the linXiv app itself); app and plugin update-checking share one entry point.

**Integrity:** the Host verifies each downloaded artifact against the sha256 in the manifest before extracting/serving; authenticity otherwise rests on HTTPS + the GitHub Releases origin (no separate artifact signing for now).

## Lifecycle / UX

Install is surfaced on the **Editor tab**: before the plugin is installed the tab shows an *"Install the LaTeX editor"* card (one-time download of editor + TeXLive, size stated, fully offline afterward) with per-artifact progress and retry-on-failure, and mounts the editor in place on success. **Management lives in Settings** — installed version + on-disk size, the unified *Check for updates*, and *Uninstall* (reclaim space). Both surfaces can trigger install, but update-check and uninstall are centralized in Settings alongside the future global app-update controls.

**Download-failure policy: restart, don't resume.** The download lands in a staging area and is promoted to the active cache only after it completes and its sha256 verifies; a failed or interrupted attempt deletes its partial data and restarts cleanly (no resume). An *existing working* cache is retained until a replacement fully verifies, then swapped and the old one deleted — so a failed update is never destructive and only a verified-complete download is ever served.

## Deferred for now: reusing a system TeXLive install

Many target users already have TeXLive installed system-wide, so reusing it to skip the download was considered. **For the current scope we will not do it** — for now linXiv **downloads and manages its own SwiftLaTeX-format TeXLive cache in its app-data dir and does not touch the user's system TeX installation.** Two reasons drive the deferral: (1) probing the user's filesystem for their TeX installation is more invasive than we want this early; and (2) it is low-value as-is, because the SwiftLaTeX WASM worker reads a by-name / `fileid`-`pkid` asset format plus its own `.fmt` format dumps, none of which a native `texmf-dist` provides.

This is a **scope decision, not a permanent one** — pending user feedback. If users with existing TeX installs ask for it, revisit reusing a system `texmf` tree (at minimum for file *contents*, accepting the `.fmt` mismatch), behind an explicit opt-in so it stays non-invasive by default.

## Consequences

- tex-brain must start producing **release artifacts** (editor build + TeXLive cache) — it ships none today (no tags; only a GitHub Pages deploy).
- The Host gains a workspace-local Tauri plugin, `tauri-plugin-texbrain`, that registers the custom URI scheme serving cached files with the required `fileid`/`pkid` headers (ported from tex-brain's `texlive://` protocol) — see ADR 0016. No SvelteKit `base` pinning is needed (the editor sits at the scheme root — see ADR 0015).
