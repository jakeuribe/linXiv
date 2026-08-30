import { apiFetch } from "./client";
import type { OrcidCandidate as UpdatedOrcidAuthor } from "../types/api";

/** One author whose ORCID was filled by a backfill pass (core's `OrcidCandidate`). */
export type { UpdatedOrcidAuthor };

// Envelope assembled inline by route/orcid.rs — no core struct to generate.
export interface OrcidBackfillResult {
  checked: number;
  updated: UpdatedOrcidAuthor[];
  /** DOIs where a source request failed (not just "no ORCID found") —
   * nonzero suggests CrossRef/OpenAlex is unreachable or rate-limiting. */
  errored: number;
}

/** Run one on-demand pass: fill ORCIDs onto authors that have none, via the
 * DOI of a paper they're linked to (CrossRef then OpenAlex per DOI). */
export async function backfillOrcids(limit?: number): Promise<OrcidBackfillResult> {
  return apiFetch<OrcidBackfillResult>("/api/orcid/backfill", {
    method: "POST",
    body: JSON.stringify(limit !== undefined ? { limit } : {}),
  });
}
