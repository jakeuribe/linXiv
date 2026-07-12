import { ApiError, isTauri } from "./client";

// The share endpoints live behind their own `share_api` Tauri command (a front
// door beside `api`, with its own ShareState + iroh node), NOT the main `/api`
// route. The iroh node only runs in the packaged/desktop app, so these calls go
// through `invoke` and are unavailable in browser dev.
async function shareApi<T>(
  method: string,
  path: string,
  body?: unknown
): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  try {
    return await invoke<T>("share_api", {
      req: { method, path, body: body ?? null },
    });
  } catch (e) {
    const err = e as { status?: number; detail?: string };
    throw new ApiError(err.status ?? 500, err.detail ?? "Request failed");
  }
}

export const sharingAvailable = isTauri;

export interface SharedSummary {
  share_id: string;
  name: string;
  paper_count: number;
  note_count: number;
  tag_count: number;
  /** Doc-file mtime (last local save/fetch) as ISO 8601; null if unreadable. */
  synced_at: string | null;
  paused: boolean;
  /** Linked local project id; null when no live local project carries this
   * SHARE_ID (received shares before first import, or linked project deleted/trashed). */
  project_fk?: number | null;
}

export type ShareDirection = "two_way" | "shared_to_local" | "local_to_shared";

export type SyncReason =
  | "paused"
  | "direction"
  | "project gone"
  | "no ticket"
  | "bad ticket"
  | "p2p offline";

export type ShareRole = "hoster" | "reader";

export interface ShareSettings {
  paused: boolean;
  direction: ShareDirection;
}

/** Summaries of every project published (shared out) from this library. */
export async function listShared(): Promise<SharedSummary[]> {
  const res = await shareApi<{ shared_projects: SharedSummary[] }>(
    "GET",
    "/api/share/projects"
  );
  return res.shared_projects;
}

/** Publish the project (if needed) and mint a one-time, pasteable ticket
 *  carrying this node's address + an unguessable capability. */
export async function createShareTicket(projectId: number): Promise<string> {
  const res = await shareApi<{ ticket: string }>(
    "POST",
    `/api/share/project/${projectId}/ticket`
  );
  return res.ticket;
}

/** Dial a ticket's sender, fetch the shared project, and store it as a
 *  read-only mirror. Returns the joined project's summary (counts only). */
export async function joinShare(
  ticket: string
): Promise<Omit<SharedSummary, "synced_at" | "paused">> {
  return shareApi("POST", "/api/share/join", { ticket });
}

/** Summaries of every shared project received via {@link joinShare}. */
export async function listReceived(): Promise<SharedSummary[]> {
  const res = await shareApi<{ received: SharedSummary[] }>(
    "GET",
    "/api/share/received"
  );
  return res.received;
}

/** Merge a received mirror into the canonical library (additive + update).
 *  Creates the linked local project on first import. */
export async function importReceived(
  shareId: string
): Promise<{ project_fk: number }> {
  return shareApi("POST", `/api/share/received/${shareId}/import`);
}

/** One-shot sync of a single share, honoring its paused/direction settings. */
export async function syncShare(
  shareId: string
): Promise<{ synced: boolean; reason?: SyncReason; role?: ShareRole }> {
  return shareApi("POST", `/api/share/${shareId}/sync`);
}

/** Drop a received mirror (+ ticket + settings). The linked local project,
 *  if imported, stays untouched. */
export async function leaveShare(shareId: string): Promise<{ left: boolean }> {
  return shareApi("POST", `/api/share/received/${shareId}/leave`);
}

/** Stop serving a published project (deletes the shared doc; SHARE_ID stays
 *  on the project so a republish reuses the same identity). */
export async function unpublishShare(
  shareId: string
): Promise<{ unpublished: boolean; share_id: string }> {
  return shareApi("POST", `/api/share/${shareId}/unpublish`);
}

export async function getShareSettings(
  shareId: string
): Promise<ShareSettings> {
  return shareApi("GET", `/api/share/${shareId}/settings`);
}

export async function updateShareSettings(
  shareId: string,
  patch: Partial<ShareSettings>
): Promise<ShareSettings> {
  return shareApi("PUT", `/api/share/${shareId}/settings`, patch);
}
