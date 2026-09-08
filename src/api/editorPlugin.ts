// Typed JS API for tauri-plugin-texbrain (EDITOR_PLUGIN_PLAN.md §2.7).
// Inlined from tauri-plugin-texbrain's guest-js (moved to tex-brain-tauri repo).
// The host imports these from src/api/editorPlugin.ts (Phase 4) — command and
// model shapes are LOCKED (§2.5); errors reject as PluginError { kind, message }.

import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

export interface PluginStatus {
  installed: boolean
  pluginVersion: string | null
  bridgeProtocol: number | null
  onDiskBytes: number | null
}

export interface UpdateCheck {
  compatible: boolean
  noCompatibleRelease: boolean
  latestVersion: string | null
  updateAvailable: boolean
  downloadBytes: number | null
}

export interface InstallProgress {
  artifact: 'editorBuild' | 'texliveCache'
  phase: 'download' | 'verify' | 'promote'
  received: number
  total: number
}

/** Shape of a rejected command's error (crate::Error custom-serialized). */
export interface PluginError {
  kind: 'offline' | 'downloadFailed' | 'checksumMismatch' | 'noCompatibleRelease' | 'busy' | 'io'
  message: string
}

/** Installed-or-not + version/size of the active Editor plugin cache. */
export async function status(): Promise<PluginStatus> {
  return await invoke('plugin:texbrain|status')
}

/** Compat-gated update check against the release feed (newest compatible release). */
export async function checkUpdates(): Promise<UpdateCheck> {
  return await invoke('plugin:texbrain|check_updates')
}

/**
 * Download + verify + promote both artifacts (also the update path — ADR 0017
 * non-destructive). Streams `texbrain://install-progress` events while running;
 * resolves to the post-install status.
 */
export async function install(): Promise<PluginStatus> {
  return await invoke('plugin:texbrain|install')
}

/** Delete the active cache to reclaim space; resolves to the post-uninstall status. */
export async function uninstall(): Promise<PluginStatus> {
  return await invoke('plugin:texbrain|uninstall')
}

/** Subscribe to per-artifact install progress. Returns the unlisten function. */
export async function onInstallProgress(
  handler: (progress: InstallProgress) => void,
): Promise<UnlistenFn> {
  return await listen<InstallProgress>('texbrain://install-progress', (event) =>
    handler(event.payload),
  )
}

/**
 * Native directory picker for the embedded editor's "Open Folder" (ADR 0018).
 * Shows the native GNOME/macOS/Windows dialog and extends the fs plugin's
 * scope recursively for the picked folder. Returns the absolute path, or null
 * if the user cancelled.
 */
export async function pickFolder(): Promise<string | null> {
  return await invoke('plugin:texbrain|pick_folder')
}

/** Bridge protocol version(s) this host supports; drives EditorPage's non-fatal
 *  warning. Must match src/install.rs::SUPPORTED_BRIDGE_PROTOCOLS — the Rust
 *  side is the compat-gating authority. */
export const SUPPORTED_BRIDGE_PROTOCOLS: readonly number[] = [1]

export function isSupportedBridgeProtocol(protocol: number): boolean {
  return SUPPORTED_BRIDGE_PROTOCOLS.includes(protocol)
}
