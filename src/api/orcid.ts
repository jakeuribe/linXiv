import { libraryFetch } from "../stores/backend.ts";
import type {
  OrcidBackfillBody,
  OrcidBackfillResponse,
  OrcidCandidate as UpdatedOrcidAuthor,
} from "../types/api";

/** One author whose ORCID was filled by a backfill pass (core's `OrcidCandidate`). */
export type { UpdatedOrcidAuthor };

/** Run one on-demand pass: fill ORCIDs onto authors that have none, via the
 * DOI of a paper they're linked to (CrossRef then OpenAlex per DOI). */
export async function backfillOrcids(limit?: number): Promise<OrcidBackfillResponse> {
  const body: OrcidBackfillBody = limit !== undefined ? { limit } : {};
  return libraryFetch<OrcidBackfillResponse>("/api/orcid/backfill", {
    method: "POST",
    body: JSON.stringify(body),
  });
}
