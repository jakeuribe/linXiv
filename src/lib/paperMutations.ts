import type { QueryClient, UseMutationOptions } from "@tanstack/react-query";
import { addPapers, createProjectWithPapers } from "../api/projects.ts";
import type { AddPapersVars, CreateProjectWithPapersVars } from "../api/projects.ts";
import { errText } from "./errText.ts";

// ---------------------------------------------------------------------------
// Invalidation registry: one owner per operation for "which cached keys go
// stale". Key sets are the UNION of what the call sites used to invalidate for
// the same operation, so every page performing it refreshes the same views.
// All keys are prefixes — react-query matches ["papers","list",sort] under
// ["papers"].
// ---------------------------------------------------------------------------

/** Keys affected by a tag edit. Tags are only ever edited through project
 *  saves and imports, so this set is folded into those operations' sets. */
export const TAG_QUERY_KEYS: readonly string[] = ["tags", "tag"];

/** Every cached key whose contents depend on which papers exist. */
export const PAPER_QUERY_KEYS: readonly string[] = [
  "papers",
  "paper",
  "projects",
  "project",
  "notes",
  "note",
  "annotations",
  ...TAG_QUERY_KEYS,
  "graph",
  "stats",
  "trash",
];

/** Keys affected by a paper mutation short of deletion: save from
 *  search/DOI/feed, import, new-version fetch, full-text index, PDF
 *  attach/detach. */
export const PAPER_MUTATION_QUERY_KEYS: readonly string[] = [
  "papers",
  "paper",
  "stats",
  ...TAG_QUERY_KEYS,
  "saved-pdfs",
];

/** Keys affected by a project mutation: create, edit (incl. its tags),
 *  archive, restore, soft/hard delete, share import. */
export const PROJECT_MUTATION_QUERY_KEYS: readonly string[] = [
  "projects",
  "project",
  ...TAG_QUERY_KEYS,
  "trash",
];

/** Keys affected by changing which papers belong to a project. */
export const PROJECT_MEMBERSHIP_QUERY_KEYS: readonly string[] = [
  "projects",
  "project",
  "papers",
];

/** Keys affected by a note create/edit/delete. */
export const NOTE_QUERY_KEYS: readonly string[] = ["notes", "note"];

/** Keys affected by an annotation create/edit/delete. */
export const ANNOTATION_QUERY_KEYS: readonly string[] = ["annotations"];

function invalidateAll(qc: QueryClient, keys: readonly string[]): Promise<void> {
  return Promise.all(keys.map((k) => qc.invalidateQueries({ queryKey: [k] }))).then(() => {});
}

/** Fan-out for a paper appearing, disappearing, or changing: soft delete,
 *  restore, hard delete, metadata save. Every such call site routes through
 *  here. */
export function invalidatePaperQueries(qc: QueryClient): Promise<void> {
  return invalidateAll(qc, PAPER_QUERY_KEYS);
}

export function invalidatePaperMutationQueries(qc: QueryClient): Promise<void> {
  return invalidateAll(qc, PAPER_MUTATION_QUERY_KEYS);
}

export function invalidateProjectMutationQueries(qc: QueryClient): Promise<void> {
  return invalidateAll(qc, PROJECT_MUTATION_QUERY_KEYS);
}

export function invalidateProjectMembershipQueries(qc: QueryClient): Promise<void> {
  return invalidateAll(qc, PROJECT_MEMBERSHIP_QUERY_KEYS);
}

export function invalidateNoteQueries(qc: QueryClient): Promise<void> {
  return invalidateAll(qc, NOTE_QUERY_KEYS);
}

export function invalidateAnnotationQueries(qc: QueryClient): Promise<void> {
  return invalidateAll(qc, ANNOTATION_QUERY_KEYS);
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

/** The page-specific hooks the shared project-picker mutations drive. */
export interface ProjectPickerActions {
  setError: (message: string | null) => void;
  /** Re-select only the failures so a retry can't re-add the rest. */
  selectFailures: (sourceIds: string[]) => void;
  /** Close the picker and clear the page's selection. */
  onDone: () => void;
  /** Clear the new-project name field (create only). */
  clearName: () => void;
}

/** Add-selection-to-project, shared by Library and Graph.
 *
 *  Partial-failure contract (chosen for both pages): never throw — resolve
 *  with the failed ids, re-select exactly those, and report the count, so a
 *  retry can't re-add the papers that already made it in. */
export function addToProjectMutationOptions(
  qc: QueryClient,
  ui: ProjectPickerActions
): UseMutationOptions<string[], Error, AddPapersVars> {
  return {
    mutationFn: addPapers,
    onMutate: () => {
      ui.setError(null);
    },
    onSettled: () => {
      invalidateProjectMembershipQueries(qc);
    },
    onSuccess: (failedIds, { sourceIds }) => {
      if (failedIds.length > 0) {
        ui.selectFailures(failedIds);
        ui.setError(partialFailureMessage(failedIds.length, sourceIds.length));
        return;
      }
      ui.setError(null);
      ui.onDone();
    },
    onError: (err) => {
      ui.setError(errText(err, "Failed to add papers to project"));
    },
  };
}

/** Create-project-with-selection, shared by Library and Graph. Same
 *  partial-failure contract as addToProjectMutationOptions. */
export function createProjectMutationOptions(
  qc: QueryClient,
  ui: ProjectPickerActions
): UseMutationOptions<string[], Error, CreateProjectWithPapersVars> {
  return {
    mutationFn: createProjectWithPapers,
    // Invalidate in onSettled, not onSuccess: the project may have been
    // created even when the mutation rejects (e.g. a paper-add request fails).
    onSettled: () => {
      invalidateProjectMembershipQueries(qc);
    },
    onSuccess: (failedIds) => {
      // The project exists either way — clear the name so a retry can't
      // create a duplicate.
      ui.clearName();
      if (failedIds.length > 0) {
        ui.selectFailures(failedIds);
        ui.setError(
          `Project created, but ${failedIds.length} paper${failedIds.length !== 1 ? "s" : ""} could not be added`
        );
        return;
      }
      ui.setError(null);
      ui.onDone();
    },
    onError: (err) => {
      ui.setError(errText(err, "Failed to create project"));
    },
  };
}
