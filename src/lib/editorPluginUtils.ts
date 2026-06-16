// Shared helpers for the editor-plugin install/update surfaces (EditorPage's
// install gate and Settings' EditorPluginSection). The two surfaces are slated
// to merge into one update entry point (ADR 0017), so their formatting/error
// idioms live here once instead of drifting as per-component copies.

import type { PluginError } from "../api/editorPlugin";

export function fmtMB(bytes: number | null | undefined): string {
  return bytes == null ? "?" : `${Math.max(1, Math.round(bytes / 1e6))} MB`;
}

export function errMessage(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (e && typeof e === "object" && "message" in e) {
    return String((e as PluginError).message);
  }
  return String(e);
}
