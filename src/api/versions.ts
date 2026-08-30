import { apiFetch } from "./client";
import type { NewVersion } from "../types/api";

/** A newly discovered arXiv version, captured into the library by a poll pass. */
export type { NewVersion };

// Envelope assembled inline by route/versions.rs — no core struct to generate.
export interface VersionCheckResult {
  checked: number;
  new_versions: NewVersion[];
}

/** Run one on-demand poll pass over the stalest `limit` saved arXiv papers. */
export async function checkNewVersions(limit?: number): Promise<VersionCheckResult> {
  return apiFetch<VersionCheckResult>("/api/versions/check", {
    method: "POST",
    body: JSON.stringify(limit !== undefined ? { limit } : {}),
  });
}

/** Papers with an un-acknowledged newly found version. */
export async function listNewVersions(): Promise<{ new_versions: NewVersion[] }> {
  return apiFetch<{ new_versions: NewVersion[] }>("/api/versions/new");
}

/** Dismiss the new-version flag for one paper. */
export async function ackNewVersion(sourceFk: number): Promise<{ ok: boolean }> {
  return apiFetch<{ ok: boolean }>("/api/versions/ack", {
    method: "POST",
    body: JSON.stringify({ source_fk: sourceFk }),
  });
}
