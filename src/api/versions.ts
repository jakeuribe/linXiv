import { libraryFetch } from "../stores/backend.ts";
import type {
  NewVersion,
  NewVersionsResponse,
  OkReceipt,
  VersionCheckResponse,
  VersionsAckBody,
  VersionsCheckBody,
} from "../types/api";

/** A newly discovered arXiv version, captured into the library by a poll pass. */
export type { NewVersion };

/** Run one on-demand poll pass over the stalest `limit` saved arXiv papers. */
export async function checkNewVersions(limit?: number): Promise<VersionCheckResponse> {
  const body: VersionsCheckBody = limit !== undefined ? { limit } : {};
  return libraryFetch<VersionCheckResponse>("/api/versions/check", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

/** Papers with an un-acknowledged newly found version. */
export async function listNewVersions(): Promise<NewVersionsResponse> {
  return libraryFetch<NewVersionsResponse>("/api/versions/new");
}

/** Dismiss the new-version flag for one paper. */
export async function ackNewVersion(sourceFk: number): Promise<OkReceipt> {
  const body: VersionsAckBody = { source_fk: sourceFk };
  return libraryFetch<OkReceipt>("/api/versions/ack", {
    method: "POST",
    body: JSON.stringify(body),
  });
}
