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

/**
 * "This operation changes what `GET /api/graph` would return."
 *
 * Marked stale but deliberately NOT refetched — see `invalidateAll`. The graph
 * is the one view in the app that must never reload on its own: a reload
 * rebuilds the force layout, and the user may have spent a while arranging it.
 * The dot on GraphPage's Refresh button (driven by `onGraphDirtying` below) is
 * how they are told there is newer data; loading it is their call.
 *
 * The claim must be per-operation: a note or annotation edit is invisible to
 * the graph, while anything that adds, removes, retitles, retags, reprojects
 * or re-authors a paper is not.
 */
export const GRAPH_QUERY_KEY = "graph";

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
  GRAPH_QUERY_KEY,
  "stats",
  "trash",
  // Reading-status rows cascade with papers/memberships and move on merge
  // (api/readingStatus.ts).
  "reading-status",
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
  // A saved paper is a new node; a new version or a tag edit changes the one
  // that is already drawn.
  GRAPH_QUERY_KEY,
];

/** Keys affected by a project mutation: create, edit (incl. its tags),
 *  archive, restore, soft/hard delete, share import. */
export const PROJECT_MUTATION_QUERY_KEYS: readonly string[] = [
  "projects",
  "project",
  ...TAG_QUERY_KEYS,
  "trash",
  // `GET /api/graph` sends each active project's name, colour and tags, and the
  // graph's Projects / Project Tags filter rows resolve their free text through
  // exactly that list.
  GRAPH_QUERY_KEY,
  // Trashing/restoring a reading list hides/reveals its status rows.
  "reading-status",
];

/** Keys affected by changing which papers belong to a project. */
export const PROJECT_MEMBERSHIP_QUERY_KEYS: readonly string[] = [
  "projects",
  "project",
  "papers",
  // Every paper node carries the ids of the active projects it belongs to
  // (crates/core/src/graph.rs sets `project_ids`), which is what the graph's
  // Projects filter matches on — so a membership change is graph data.
  GRAPH_QUERY_KEY,
  // Removing a paper from a reading list cascades its status row away.
  "reading-status",
];

/** Keys affected by an author rename, delete, merge, or a paper↔author
 *  link/unlink (reassign is unlink+link) — the one paper-shaped
 *  operation class that had no owner here, so each call site spelled out its
 *  own key list. Author nodes and the paper->author edges the graph's Author
 *  filter matches through come from AUTHOR / PAPER_TO_AUTHOR, so a merge or a
 *  rename redraws the canvas. No graph-visible key here is one another page is
 *  guaranteed to have cached, which is why the marker matters. */
export const AUTHOR_MUTATION_QUERY_KEYS: readonly string[] = [
  "authors",
  "author",
  "author-merge-candidates",
  GRAPH_QUERY_KEY,
];

/** Keys affected by a note create/edit/delete. */
export const NOTE_QUERY_KEYS: readonly string[] = ["notes", "note"];

/** Keys affected by an annotation create/edit/delete. */
export const ANNOTATION_QUERY_KEYS: readonly string[] = ["annotations"];

// --- Graph staleness -------------------------------------------------------
// GraphPage flags its Refresh button by watching the query cache for
// `invalidate` events, but react-query only emits one per query that is
// ACTUALLY IN THE CACHE: `invalidateQueries({queryKey: ["authors"]})` from a
// page that never mounted an ["authors"] query notifies nobody, and the graph
// silently keeps drawing the old data. That page therefore keeps a ["stats"]
// query alive on purpose — but "stats" is not in every set above, so the
// guarantee only ever covered some of the operations.
//
// The registry already knows which operations touch the graph; this is it
// saying so directly, independent of what any other page happens to have
// cached. GraphPage still keeps the cache subscription as well, for the sites
// that invalidate without coming through here.
type GraphDirtyListener = () => void;
const graphDirtyListeners = new Set<GraphDirtyListener>();

/** Subscribe to "an operation just changed what `/api/graph` would return".
 *  Returns the unsubscribe. */
export function onGraphDirtying(listener: GraphDirtyListener): () => void {
  graphDirtyListeners.add(listener);
  return () => {
    graphDirtyListeners.delete(listener);
  };
}

function invalidateAll(qc: QueryClient, keys: readonly string[]): Promise<void> {
  // Announced before the awaits: the flag is about data that has already
  // changed on the backend, not about the refetches finishing.
  if (keys.includes(GRAPH_QUERY_KEY)) {
    for (const listener of [...graphDirtyListeners]) listener();
  }
  return Promise.all(
    keys.map((k) =>
      // `refetchType: "none"` for the graph alone: mark it stale, do not fetch.
      //
      // Everything else here SHOULD refresh on its own, and does. The graph must
      // not, and the default would: keys match by prefix, so ["graph"] matches
      // the ["graph", excludeSingleAuthors] entry GraphPage holds, and
      // invalidateQueries defaults to refetchType "active" — which refetches a
      // mounted, enabled query at once, whatever its staleTime. GraphPage is
      // mounted for the rest of the session once /graph has been visited (the
      // app shell keeps it alive behind `display: none`), so retitling a paper
      // from the Library would silently re-fetch the payload, and a new payload
      // rebuilds the simulation — re-annealing from alpha 1 and drifting the
      // arrangement the user made, with no action of theirs to explain it. Mid
      // drag it would also destroy the grabbed node under the gesture.
      qc.invalidateQueries(
        k === GRAPH_QUERY_KEY ? { queryKey: [k], refetchType: "none" } : { queryKey: [k] }
      )
    )
  ).then(() => {});
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

export function invalidateAuthorQueries(qc: QueryClient): Promise<void> {
  return invalidateAll(qc, AUTHOR_MUTATION_QUERY_KEYS);
}

export function invalidateNoteQueries(qc: QueryClient): Promise<void> {
  return invalidateAll(qc, NOTE_QUERY_KEYS);
}

export function invalidateAnnotationQueries(qc: QueryClient): Promise<void> {
  return invalidateAll(qc, ANNOTATION_QUERY_KEYS);
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
