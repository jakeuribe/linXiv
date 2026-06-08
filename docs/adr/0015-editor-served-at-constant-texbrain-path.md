# Editor address is environment-derived; production uses a custom URI scheme (cross-origin)

---
Status: accepted — confirmed by spike `wl6398iw4` (verified against `tauri 2.11.2` source, maintainer issues, and in-repo precedent): runtime-downloaded content **cannot** be served same-origin under `tauri://localhost`, so a custom scheme (separate origin) is required. Note: a *bundled* SPA can be served same-origin (that is how tex-brain serves its own editor), but our editor is runtime-downloaded, which forecloses that route.
---


The embedded editor's iframe address is **derived from `import.meta.env.DEV`**, not a hard-coded same-origin path:

- **dev:** `EDITOR_SRC = http://localhost:5173/editor`, `EDITOR_ORIGIN = http://localhost:5173` — the editor's own Vite dev server.
- **prod:** `EDITOR_SRC = texbrain://localhost/editor`, `EDITOR_ORIGIN = texbrain://localhost` — served from the plugin cache by the `tauri-plugin-texbrain` custom scheme (ADR 0016).

Only the `/editor` **path** is constant; the **origin/scheme is environment-derived**. The host↔editor postMessage bridge is **pinned cross-origin** (never `'*'`) and already works this way — it's how the embed has run throughout development. The SwiftLaTeX worker's `credentials:'same-origin'` asset fetches are satisfied because the editor document and its assets share the editor's *own* origin (`localhost:5173` in dev, `texbrain://localhost` in prod) — same-origin is required between the editor and its assets, not between the editor and the host SPA.

**Why env-derived, not a constant same-origin path:** runtime-*downloaded* plugin content cannot live under the bundled app origin (`tauri://localhost`, served from `frontendDist`); Tauri's mechanism for serving dynamic/cached content is a **custom URI scheme, which is a separate origin**. A relative same-origin path therefore cannot address it. An earlier same-session draft proposed a constant `/texbrain/editor` path with a dev Vite proxy for same-origin dev — **dropped**, because (a) prod cannot match it, and (b) a same-origin-dev / cross-origin-prod split would hide origin-sensitive bugs until release. Dev is intentionally **also cross-origin** (no proxy) so the dev origin model mirrors prod.

The signal is **`import.meta.env.DEV`**, not `isTauri`, because `tauri dev` is a Tauri webview running in dev and must use the dev address (`isTauri` would misclassify it as prod).

## Consequences

- **No SvelteKit `base` pinning needed.** The editor lives at the **root** of its origin (dev-server root / custom-scheme root), so `base` stays as-is — the `texbrain://` scheme provides the namespace. This **removes the earlier "pin base to `/texbrain`" requirement**.
- `editorConfig.ts` derives both `EDITOR_SRC` and `EDITOR_ORIGIN` from `import.meta.env.DEV` (replacing the current `isTauri` switch).
- Dev ports: host **5180**, editor **5173** (dev-only; the packaged app has no frontend ports).
- **Cross-origin constraints (surfaced by the spike):** the editor and host SPA are separate origins, so they share **no** `localStorage`/`sessionStorage`/cookies — all host↔editor state crosses via the postMessage bridge (already the case). The `texbrain://` scheme must be permitted by the webview **CSP `frame-src`/`child-src`** and declared in **Tauri capabilities**.
- *(Trade-off noted: cross-origin dev shows benign `postMessage` origin-mismatch console warnings — accepted for dev/prod parity. The proxy could be reinstated for same-origin dev if those warnings prove bothersome, at the cost of parity.)*

Evidence (spike `wl6398iw4`): `tauri-2.11.2/src/protocol/tauri.rs:212-224`, `manager/webview.rs:267-278`, `app.rs:2113-2120`; `tex-brain-tauri/src-tauri/src/main.rs:100` (separate `texlive://` scheme); tauri-apps/tauri discussion #8285, issue #6869.
