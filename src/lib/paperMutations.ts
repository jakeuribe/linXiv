import type { QueryClient } from "@tanstack/react-query";

/** Every cached key whose contents depend on which papers exist. These are
 *  prefixes — react-query matches ["papers","list",sort] under ["papers"]. */
export const PAPER_QUERY_KEYS: readonly string[] = [
  "papers",
  "paper",
  "projects",
  "project",
  "notes",
  "note",
  "annotations",
  "tags",
  "tag",
  "graph",
  "stats",
  "trash",
];

/** Keys affected by changing which papers belong to a project. */
export const PROJECT_MEMBERSHIP_QUERY_KEYS: readonly string[] = ["projects", "project"];

function invalidateAll(qc: QueryClient, keys: readonly string[]): Promise<void> {
  return Promise.all(keys.map((k) => qc.invalidateQueries({ queryKey: [k] }))).then(() => {});
}

/** Fan-out for a paper appearing or disappearing: soft delete, restore, hard
 *  delete. Every such call site routes through here. */
export function invalidatePaperQueries(qc: QueryClient): Promise<void> {
  return invalidateAll(qc, PAPER_QUERY_KEYS);
}

export function invalidateProjectMembershipQueries(qc: QueryClient): Promise<void> {
  return invalidateAll(qc, PROJECT_MEMBERSHIP_QUERY_KEYS);
}

export interface ReadingStatusRemover {
  remove: (sourceId: string) => void;
}

/** Drop client-only state keyed by source_id. Permanent removal only — a soft
 *  delete must keep it so a trash → restore round-trip preserves the status. */
export function forgetPurgedPapers(sourceIds: string[], store: ReadingStatusRemover): void {
  for (const sid of sourceIds) store.remove(sid);
}

/** Shared wording for the partial-failure contract: some papers added, some not. */
export function partialFailureMessage(failedCount: number, totalCount: number): string {
  const plural = totalCount !== 1 ? "s" : "";
  return `${failedCount} of ${totalCount} paper${plural} could not be added`;
}
