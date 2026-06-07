// editorConfig.ts
// -----------------------------------------------------------------------------
// Host-side configuration for the embedded TeXbrain editor: the iframe origin
// and src. The EditorBridge wire protocol + EditorBridgeClient + NoopFsResponder
// + pushThemeToEditor live in src/lib/editorBridge.ts (and editorBridgeTypes.ts)
// — this module only owns the WHERE (origin/src) and re-exports the bridge for
// convenient single-import wiring from EditorPage.
//
// ADR 0015 (LOCKED): the address is derived from `import.meta.env.DEV` — NOT
// `isTauri`, which would misclassify `tauri dev` (a Tauri webview running in
// dev MUST use the dev address):
//   dev:  http://localhost:5173/editor — the editor's own Vite dev server
//         (the host moved to 5180; ADR 0015 "host 5180, editor 5173"). Dev is
//         intentionally cross-origin like prod — no proxy — so origin bugs
//         can't hide until release.
//   prod: <texbrain scheme origin>/editor — the runtime-downloaded Editor
//         plugin served by tauri-plugin-texbrain's custom scheme (ADR 0017/0016).
// Only the /editor PATH is constant; the origin/scheme is environment-derived.
// -----------------------------------------------------------------------------

/**
 * Prod custom-scheme origin for a Tauri-registered scheme. PLATFORM-DEPENDENT
 * (plan finding 11, verified against tauri 2.11.2 `plugin.rs`): macOS/iOS/Linux
 * resolve a registered scheme to `<scheme>://localhost`, while Windows
 * (WebView2) maps it to `http://<scheme>.localhost`. EDITOR_ORIGIN is used BOTH
 * as the postMessage targetOrigin AND as EditorBridgeClient's strict
 * `event.origin` pin — the wrong form silently rejects every guest message and
 * the handshake never completes, so this MUST match the iframe's real origin on
 * the running platform. UA sniffing is dependable inside the Tauri webview
 * (WebView2 always reports a Windows UA).
 */
export function schemeOrigin(scheme: string): string {
  const isWindows =
    typeof navigator !== "undefined" && navigator.userAgent.includes("Windows");
  return isWindows ? `http://${scheme}.localhost` : `${scheme}://localhost`;
}

/**
 * Origin of the embedded TeXbrain editor. Used BOTH as the postMessage
 * targetOrigin (host -> guest) AND as the allowed event.origin (guest -> host).
 * Never '*' — the bridge is first-party app-to-app, so origins are pinned
 * (EditorBridgeClient rejects any message whose event.origin !== this value).
 */
export const EDITOR_ORIGIN: string = import.meta.env.DEV
  ? "http://localhost:5173"
  : schemeOrigin("texbrain");

/**
 * The iframe `src`. Only the `/editor` path is constant (ADR 0015). We pass the
 * HOST's own origin as `?host=` so the guest knows where to post `texbrain:ready`
 * and which origin to accept messages from. This is essential in prod: the host
 * page is a custom scheme (`tauri://localhost`) and the referrer to a
 * `texbrain://` iframe is stripped/opaque, so the guest's old referrer-derived
 * origin was wrong and the bridge silently never handed off. The host knows its
 * own origin unambiguously via `window.location.origin`.
 */
export const EDITOR_SRC: string = `${EDITOR_ORIGIN}/editor?host=${encodeURIComponent(
  typeof window !== "undefined" ? window.location.origin : ""
)}`;

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
