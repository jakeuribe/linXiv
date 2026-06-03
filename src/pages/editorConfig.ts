// editorConfig.ts
// -----------------------------------------------------------------------------
// Host-side configuration for the embedded TeXbrain editor: the iframe origin
// and src. The EditorBridge wire protocol + EditorBridgeClient + NoopFsResponder
// + pushThemeToEditor already live in src/lib/editorBridge.ts (and
// editorBridgeTypes.ts) — this module only owns the where (origin/src) and
// re-exports the bridge for convenient single-import wiring from EditorPage.
//
// Nothing here is imported anywhere yet; the page stays additive until the
// register-editor-route-and-nav section wires it into the router/sidebar.
// -----------------------------------------------------------------------------

import { isTauri } from "../api/client";

/**
 * Origin of the embedded TeXbrain editor. Used BOTH as the postMessage
 * targetOrigin (host -> guest) AND as the allowed event.origin (guest -> host).
 * Never '*' — the bridge is first-party app-to-app, so origins are pinned
 * (EditorBridgeClient rejects any message whose event.origin !== this value).
 *
 * Dev (browser/Vite, !isTauri): TeXbrain's Vite dev server at :5173 — a
 *   distinct origin, so host<->guest postMessage is genuinely cross-origin.
 * Prod (inside the Tauri webview, isTauri): same-origin bundled path under
 *   tauri://localhost, so event.origin === window.location.origin.
 *
 * Using `isTauri` (the __TAURI_INTERNALS__ runtime check from api/client)
 * mirrors how the rest of the GUI distinguishes the bundled webview from
 * browser dev (see api/client.ts), rather than import.meta.env.
 *
 * TODO(prod): the prod value depends on two not-yet-ready sections —
 *   - `bundle-texbrain-static-as-resource` (ship TeXbrain's built /editor +
 *     /swiftlatex/* + /texlive/* into the Tauri webview asset root), and
 *   - `base-path pinning` (TeXbrain's SvelteKit `base` must be pinned to
 *     '/texbrain' so its asset URLs resolve under that prefix).
 * Until then prod is a best-effort placeholder; do not rely on it.
 */
export const EDITOR_ORIGIN: string = isTauri
  ? window.location.origin
  : "http://localhost:5173";

/**
 * The iframe `src`. Dev points at TeXbrain's Vite dev server /editor route;
 * prod points at the same-origin bundled path.
 *
 * TODO(prod): '/texbrain/editor' assumes bundle-texbrain-static-as-resource has
 * placed the built editor under that path and that TeXbrain's SvelteKit base is
 * pinned to '/texbrain' (base-path pinning). Both are separate, not-yet-ready
 * sections.
 */
export const EDITOR_SRC: string = isTauri ? "/texbrain/editor" : "http://localhost:5173/editor";

// Re-export the bridge so EditorPage can wire everything from a single import.
export {
  EditorBridgeClient,
  NoopFsResponder,
  pushThemeToEditor,
} from "../lib/editorBridge";
export type {
  ThemePushState,
  FsResponder,
  EditorBridgeHandlers,
  DocOpenPayload,
} from "../lib/editorBridge";
