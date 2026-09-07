import { libraryFetch } from "../stores/backend.ts";
import type {
  NewVersion,
  NewVersionsResponse,
  OkReceipt,
  VersionCheckResponse,
} from "../types/api";

/** A newly discovered arXiv version, captured into the library by a poll pass. */
export type { NewVersion };

/** Run one on-demand poll pass over the stalest `limit` saved arXiv papers. */
export async function checkNewVersions(limit?: number): Promise<VersionCheckResponse> {
  return libraryFetch<VersionCheckResponse>("/api/versions/check", {
    method: "POST",
    body: JSON.stringify(limit !== undefined ? { limit } : {}),
  });
}

/** Papers with an un-acknowledged newly found version. */
export async function listNewVersions(): Promise<NewVersionsResponse> {
  return libraryFetch<NewVersionsResponse>("/api/versions/new");
}

/** Dismiss the new-version flag for one paper. */
export async function ackNewVersion(sourceFk: number): Promise<OkReceipt> {
  return libraryFetch<OkReceipt>("/api/versions/ack", {
    method: "POST",
    body: JSON.stringify({ source_fk: sourceFk }),
  });
}
