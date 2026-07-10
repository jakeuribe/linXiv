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
  annotation_count: number;
  tag_count: number;
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
 *  read-only mirror. Returns the joined project's summary. */
export async function joinShare(ticket: string): Promise<SharedSummary> {
  return shareApi<SharedSummary>("POST", "/api/share/join", { ticket });
}

/** Summaries of every shared project received via {@link joinShare}. */
export async function listReceived(): Promise<SharedSummary[]> {
  const res = await shareApi<{ received: SharedSummary[] }>(
    "GET",
    "/api/share/received"
  );
  return res.received;
}
