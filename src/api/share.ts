import { ApiError, isTauri } from "./client";
import type {
  SummaryRow as SharedSummary,
  MemberRow as ShareMember,
  ShareSettings,
  SyncDirection as ShareDirection,
  SharedProjectsListing,
  ReceivedListing,
  MembersListing,
  TicketMinted,
  ImportedReceipt,
  UnlinkedReceipt,
  LeftReceipt,
  UnpublishedReceipt,
  PublishedReceipt,
  RoleChanged,
  RekeyedReceipt,
  RemovedReceipt,
  RevokedReceipt,
  SharedPdfSaved,
  MemberCode,
  InviteMinted,
} from "../types/api";

export type { SharedSummary, ShareMember, ShareSettings, ShareDirection };

/** Deliberately narrower than lib/errText: only ApiError messages surface in
 * the sharing UI — any other exception falls back to the generic string
 * rather than leaking its raw message. */
export function shareErrText(e: unknown): string {
  return e instanceof ApiError ? e.message : "Unexpected sharing error";
}

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

// Most envelope types are generated from the Rust structs in
// crates/server/src/route/share.rs (aliased above); only shapes with no exact
// Rust twin (JoinResult, syncShare's json! envelope, ReceivedPaper) stay
// hand-written below.

export type MemberRole = "hoster" | "editor" | "viewer";

export type SyncReason =
  | "paused"
  | "direction"
  | "project gone"
  | "no ticket"
  | "bad ticket"
  | "p2p offline"
  | "revoked or awaiting key"
  | "awaiting first sync"
  | "no key for any content";

export type ShareRole = "hoster" | "reader";

/** Summaries of every project published (shared out) from this library. */
export async function listShared(): Promise<SharedSummary[]> {
  const res = await shareApi<SharedProjectsListing>(
    "GET",
    "/api/share/projects"
  );
  return res.shared_projects;
}

/** Publish the project (if needed) and mint a one-time, pasteable ticket
 *  carrying this node's address + an unguessable capability. */
export async function createShareTicket(projectId: number): Promise<string> {
  const res = await shareApi<TicketMinted>(
    "POST",
    `/api/share/project/${projectId}/ticket`
  );
  return res.ticket;
}

/** Outcome of {@link joinShare}. `pending` means the invite was accepted but
 *  its host was unreachable: it is saved and finishes syncing on a later pass,
 *  so there is no name or counts yet. {@link listReceived} lists it with
 *  `pending` set until that first sync lands. */
export type JoinResult =
  | ({ pending?: false } & Omit<SharedSummary, "synced_at" | "paused">)
  | { pending: true; share_id: string; e2ee: true; reason: string };

/** Shown once a join has been running long enough to look stuck. An offline
 *  host cannot be detected quickly — QUIC has no connection-refused, so the
 *  dial can only time out (15s, `DIAL_TIMEOUT` in the p2p crate). Deliberately
 *  conditional: a host that *refuses* this device fails and saves nothing, so
 *  this must not promise the invite was kept. */
export const JOIN_SLOW_HINT =
  "Connecting to the host… If they are offline this takes about 15 seconds, and the invite is saved to finish syncing later.";

/** Dial a ticket's sender, fetch the shared project, and store it as a
 *  read-only mirror. Returns the joined project's summary (counts only), or a
 *  `pending` result when an e2ee invite's host could not be reached. */
export async function joinShare(ticket: string): Promise<JoinResult> {
  return shareApi("POST", "/api/share/join", { ticket });
}

/** Summaries of every shared project received via {@link joinShare}. */
export async function listReceived(): Promise<SharedSummary[]> {
  const res = await shareApi<ReceivedListing>("GET", "/api/share/received");
  return res.received;
}

/** Merge a received mirror into the canonical library (additive + update).
 *  Creates the linked local project on first import. */
export async function importReceived(shareId: string): Promise<ImportedReceipt> {
  return shareApi("POST", `/api/share/received/${shareId}/import`);
}

/** Detach the linked local project from a received share. Membership, mirror,
 *  and the local project all stay; interval sync keeps the mirror fresh but
 *  stops importing until {@link importReceived} creates a new link. */
export async function unlinkShare(shareId: string): Promise<UnlinkedReceipt> {
  return shareApi("POST", `/api/share/received/${shareId}/unlink`);
}

/** One-shot sync of a single share, honoring its paused/direction settings. */
export async function syncShare(
  shareId: string
): Promise<{
  synced: boolean;
  reason?: SyncReason;
  role?: ShareRole;
  /** Notes/annotations skipped because their key is revoked or not yet received. */
  undecryptable?: number;
  e2ee?: boolean;
  /** The sync ran but the mirror is still empty — the host has not answered. */
  pending?: boolean;
  /** Reader leg: commits decrypted and applied this pass. */
  applied?: number;
  /** Reader leg: commits fetched with no key for their epoch. `no_key > 0` with
   *  nothing applied means content sealed to an epoch this device never joined
   *  — typically published before the invite; retrying cannot re-key it. */
  no_key?: number;
  /** Reader leg: commits that failed to decrypt for any other reason. */
  failed?: number;
  /** Hoster leg: devices this share is currently granted to. */
  members?: number;
}> {
  return shareApi("POST", `/api/share/${shareId}/sync`);
}

/** Drop a received mirror (+ ticket + settings) and forget the p2p
 *  registration behind it, so a rejoin adopts from scratch instead of reusing
 *  the old document. The linked local project, if imported, stays untouched.
 *  `forgotten: false` means the node was offline and the registration
 *  survived — a rejoin would reuse the old doc. */
export async function leaveShare(shareId: string): Promise<LeftReceipt> {
  return shareApi("POST", `/api/share/received/${shareId}/leave`);
}

/** Stop serving a published project (deletes the shared doc; SHARE_ID stays
 *  on the project so a republish reuses the same identity). */
export async function unpublishShare(
  shareId: string
): Promise<UnpublishedReceipt> {
  return shareApi("POST", `/api/share/${shareId}/unpublish`);
}

/** Rebinds the p2p node against whatever relay settings are currently saved
 *  (Settings → Sharing), without restarting the app. Save the settings first
 *  via `updateSettings`, then call this. */
export async function reconnectRelay(): Promise<void> {
  await shareApi("POST", "/api/share/relay/reconnect");
}

/** This device's pasteable membership code — sent to a host to be invited
 *  to an encrypted share. */
export async function memberCode(): Promise<string> {
  const res = await shareApi<MemberCode>("GET", "/api/share/member_code");
  return res.code;
}

/** Publish the project as an end-to-end encrypted share. No ticket — access
 *  is granted per-device via {@link inviteMember}. */
export async function publishSecure(
  projectId: number
): Promise<PublishedReceipt> {
  return shareApi("POST", `/api/share/project/${projectId}/publish_secure`);
}

/** Grant a device access to a hosted e2ee share and mint its pasteable
 *  invite string. */
export async function inviteMember(
  shareId: string,
  opts: { memberCode: string; role: Exclude<MemberRole, "hoster">; name?: string }
): Promise<string> {
  const res = await shareApi<InviteMinted>(
    "POST",
    `/api/share/${shareId}/invite`,
    { member_code: opts.memberCode, role: opts.role, name: opts.name }
  );
  return res.invite;
}

/** Members of a hoster-owned e2ee share (the hoster entry is this device). */
export async function listMembers(shareId: string): Promise<ShareMember[]> {
  const res = await shareApi<MembersListing>(
    "GET",
    `/api/share/${shareId}/members`
  );
  return res.members;
}

/** Change an invited member's role on a hosted e2ee share (viewer ↔ editor;
 *  admin is keyhive-supported but app-deferred, the route rejects it). */
export async function setMemberRole(
  shareId: string,
  memberId: string,
  role: Exclude<MemberRole, "hoster">
): Promise<RoleChanged> {
  return shareApi("POST", `/api/share/${shareId}/member/${memberId}/role`, {
    role,
  });
}

/** Re-encrypt a hosted encrypted share's whole history (and its PDF blobs)
 *  under the current key, then republish. Repairs members who joined after the
 *  content was already encrypted and so can decrypt none of it — the symptom is
 *  their sync reporting `no_key > 0` with `applied` stuck at 0. */
export async function rekeyShare(shareId: string): Promise<RekeyedReceipt> {
  return shareApi("POST", `/api/share/${shareId}/rekey`);
}

/** Revoke a member and drop their row entirely, so re-inviting the same device
 *  starts clean. Use over {@link revokeMember} when the invite is being redone
 *  rather than withdrawn. */
export async function removeMember(
  shareId: string,
  memberId: string
): Promise<RemovedReceipt> {
  return shareApi("POST", `/api/share/${shareId}/member/${memberId}/remove`);
}

/** Revoke a member: stops receiving future updates; content already synced
 *  stays on their device. */
export async function revokeMember(
  shareId: string,
  memberId: string
): Promise<RevokedReceipt> {
  return shareApi("POST", `/api/share/${shareId}/revoke`, {
    member_id: memberId,
  });
}

/** Fields the share page consumes for one paper in a received mirror. */
export interface ReceivedPaper {
  source_id: string;
  title: string;
  has_pdf: boolean;
}

/** Papers of one received mirror. */
export async function listReceivedPapers(
  shareId: string
): Promise<ReceivedPaper[]> {
  const res = await shareApi<{ papers: ReceivedPaper[] }>(
    "GET",
    `/api/share/received/${shareId}`
  );
  return res.papers;
}

/** Fetch + decrypt one shared PDF blob and save it to the managed PDF dir. */
export async function downloadSharedPdf(
  shareId: string,
  sourceId: string
): Promise<SharedPdfSaved> {
  return shareApi("POST", `/api/share/${shareId}/pdf`, {
    source_id: sourceId,
  });
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
