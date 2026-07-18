import { isTauri } from "../api/client.ts";

// Interface zoom bounds. 100% is the unscaled baseline; the range mirrors what
// a typical desktop browser offers via Ctrl +/− and what the underlying webview
// engines (WebView2, WebKitGTK, WKWebView) handle reliably.
export const MIN_ZOOM = 0.5;
export const MAX_ZOOM = 2.0;
export const ZOOM_STEP = 0.1;
export const DEFAULT_ZOOM = 1.0;

/**
 * Clamp to the supported range and round to whole percentage points. Rounding
 * matters because repeated ±0.1 steps accumulate float error (0.7 → 0.69999…),
 * which would otherwise make equality checks like `zoom === DEFAULT_ZOOM` fail.
 */
export function clampZoom(zoom: number): number {
  if (!Number.isFinite(zoom)) return DEFAULT_ZOOM;
  const rounded = Math.round(zoom * 100) / 100;
  return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, rounded));
}

/**
 * Apply the zoom factor to the whole interface.
 *
 * In the desktop app we use the webview's native zoom, which scales every
 * pixel uniformly (exactly like browser zoom) — the only mechanism that also
 * scales the px-based icons and inline sizes scattered through the UI. The
 * webview resets to 100% on each launch, so callers re-apply the persisted
 * value on boot. In the browser dev server there is no Tauri webview, so we
 * fall back to the CSS `zoom` property. Only one mechanism is ever active, so
 * the two never compound.
 */
export function applyZoom(zoom: number): void {
  const factor = clampZoom(zoom);
  if (isTauri) {
    void import("@tauri-apps/api/webview")
      .then(({ getCurrentWebview }) => getCurrentWebview().setZoom(factor))
      .catch((err) => console.error("Failed to set webview zoom", err));
  } else {
    document.documentElement.style.setProperty("zoom", String(factor));
  }
}
