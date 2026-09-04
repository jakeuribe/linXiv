// Remote Library Backend management + the byte-lane PDF fetch — thin wrappers
// over the Tauri `remote_*` commands (src-tauri/src/remote_backend.rs). All of
// this is desktop-only: the iroh transport lives in-process.
import { isTauri, mapRemoteError, type RemoteBackend } from "./client";

export type { RemoteBackend } from "./client";

export const remoteAvailable = isTauri;

async function cmd<T>(
  name: string,
  args?: Record<string, unknown>
): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  try {
    return await invoke<T>(name, args);
  } catch (e) {
    throw mapRemoteError(e);
  }
}

export function listRemoteBackends(): Promise<RemoteBackend[]> {
  return cmd<RemoteBackend[]>("remote_backends_list");
}

/** Validates (Rust-side) that `address` parses as a Node Address; stores only,
 *  never dials — reachability is a per-request question. */
export function addRemoteBackend(
  label: string,
  address: string
): Promise<RemoteBackend> {
  return cmd<RemoteBackend>("remote_backend_add", { label, address });
}

export function removeRemoteBackend(id: string): Promise<void> {
  return cmd<void>("remote_backend_remove", { id });
}

/** This device's member code (transport endpoint id) — what a node operator
 *  adds to their Member List to admit this device. */
export function remoteMemberCode(): Promise<string> {
  return cmd<string>("remote_member_code");
}

/** Fetch (or serve from cache) a remote paper's PDF; returns the local
 *  absolute path for the existing path-based viewer machinery. */
export function remotePdfPath(
  backendId: string,
  sourceId: string,
  version?: number
): Promise<string> {
  return cmd<string>("remote_pdf", { backendId, sourceId, version });
}
