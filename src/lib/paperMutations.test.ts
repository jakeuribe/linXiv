// Run: node --experimental-transform-types --test src/lib/paperMutations.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { QueryClient, QueryObserver } from "@tanstack/react-query";
import {
  PAPER_QUERY_KEYS,
  PAPER_MUTATION_QUERY_KEYS,
  PROJECT_MUTATION_QUERY_KEYS,
  PROJECT_MEMBERSHIP_QUERY_KEYS,
  AUTHOR_MUTATION_QUERY_KEYS,
  NOTE_QUERY_KEYS,
  ANNOTATION_QUERY_KEYS,
  GRAPH_QUERY_KEY,
  invalidatePaperQueries,
  invalidatePaperMutationQueries,
  invalidateProjectMutationQueries,
  invalidateProjectMembershipQueries,
  invalidateAuthorQueries,
  invalidateNoteQueries,
  invalidateAnnotationQueries,
  onGraphDirtying,
  forgetPurgedPapers,
  partialFailureMessage,
  addToProjectMutationOptions,
  createProjectMutationOptions,
  type ReadingStatusRemover,
  type ProjectPickerActions,
} from "./paperMutations.ts";

/** Seeds one cached entry per key and returns the keys left stale afterwards. */
async function staleAfter(
  keys: string[][],
  run: (qc: QueryClient) => Promise<void>
): Promise<Set<string>> {
  const qc = new QueryClient();
  for (const key of keys) qc.setQueryData(key, "cached");
  await run(qc);
  const stale = new Set<string>();
  for (const entry of qc.getQueryCache().getAll()) {
    if (entry.state.isInvalidated) stale.add(JSON.stringify(entry.queryKey));
  }
  return stale;
}

// The defect: a paper delete from one page refreshed fewer views than the same
// delete from another. Every paper-existence key must go stale together.
test("a paper delete invalidates every paper-existence view", async () => {
  const seeded = PAPER_QUERY_KEYS.map((k) => [k]);
  const stale = await staleAfter(seeded, invalidatePaperQueries);

  for (const key of PAPER_QUERY_KEYS) {
    assert.ok(stale.has(JSON.stringify([key])), `["${key}"] was left fresh`);
  }
  // The views the Library delete used to miss.
  for (const key of ["graph", "tags", "tag", "stats", "trash", "notes"]) {
    assert.ok(PAPER_QUERY_KEYS.includes(key), `${key} missing from the registry`);
  }
});

// The graph is the one view that must never reload on its own: a new payload
// rebuilds the force simulation, which re-anneals from alpha 1 and drifts the
// arrangement the user made. Keys match by PREFIX and invalidateQueries defaults
// to refetchType "active", so a plain invalidate would refetch the
// ["graph", excludeSingleAuthors] entry GraphPage holds.
//
// "Active" means "has a subscribed observer", which is what useQuery creates —
// so these mount a real QueryObserver. Without one, a cache entry is inactive
// and would sit out the refetch whatever refetchType said, which would let the
// graph assertion below pass for entirely the wrong reason.
async function mounted(qc: QueryClient, queryKey: unknown[]) {
  let fetches = 0;
  const observer = new QueryObserver(qc, {
    queryKey,
    queryFn: async () => {
      fetches++;
      return "payload";
    },
    staleTime: Infinity,
  });
  const unsubscribe = observer.subscribe(() => {});
  await observer.refetch();
  return { count: () => fetches, unsubscribe, observer };
}

test("a graph-dirtying operation marks the graph stale without refetching it", async () => {
  const qc = new QueryClient();
  const graph = await mounted(qc, ["graph", false]);
  assert.equal(graph.count(), 1, "the initial load");

  await invalidatePaperQueries(qc);
  await new Promise((r) => setTimeout(r, 20));

  assert.equal(graph.count(), 1, "invalidation must not refetch the graph");
  assert.ok(
    qc.getQueryState(["graph", false])?.isInvalidated,
    "…but it must still be marked stale, so the state is honest"
  );
  graph.unsubscribe();
});

// The other half of the same contract: everything that is NOT the graph keeps
// refreshing on its own, which is the whole point of the registry. If this ever
// fails the same way, the exemption above has leaked to every key.
test("a graph-dirtying operation still refetches the other views", async () => {
  const qc = new QueryClient();
  const papers = await mounted(qc, ["papers", "list"]);
  assert.equal(papers.count(), 1);

  await invalidatePaperQueries(qc);
  await new Promise((r) => setTimeout(r, 20));

  assert.equal(papers.count(), 2, "papers must reload without being asked");
  papers.unsubscribe();
});

test("invalidation matches nested keys by prefix", async () => {
  const stale = await staleAfter(
    [["papers", "list", "title_asc"], ["paper", "sfk", 7], ["tag", "ml"], ["settings"]],
    invalidatePaperQueries
  );

  assert.ok(stale.has(JSON.stringify(["papers", "list", "title_asc"])));
  assert.ok(stale.has(JSON.stringify(["paper", "sfk", 7])));
  assert.ok(stale.has(JSON.stringify(["tag", "ml"])));
  // Unrelated caches must survive.
  assert.ok(!stale.has(JSON.stringify(["settings"])));
});

// Each registry set is the union of what its call sites used to invalidate —
// these pin the members a single divergent site contributed, so a future trim
// can't silently reintroduce the per-page divergence.
test("registry sets keep their union members", () => {
  for (const key of ["saved-pdfs", "tags", "tag", "stats", "paper"]) {
    assert.ok(PAPER_MUTATION_QUERY_KEYS.includes(key), `${key} missing from paper mutation set`);
  }
  for (const key of ["trash", "tags", "tag", "project"]) {
    assert.ok(PROJECT_MUTATION_QUERY_KEYS.includes(key), `${key} missing from project mutation set`);
  }
  assert.ok(PROJECT_MEMBERSHIP_QUERY_KEYS.includes("papers"), "papers missing from membership set");
});

test("project membership invalidation stays narrow", async () => {
  const stale = await staleAfter(
    [["projects"], ["project", "3"], ["graph"], ["stats"]],
    invalidateProjectMembershipQueries
  );

  assert.ok(stale.has(JSON.stringify(["projects"])));
  assert.ok(stale.has(JSON.stringify(["project", "3"])));
  assert.ok(!stale.has(JSON.stringify(["stats"])));
  // ["graph"] is not narrowness, it is the graph-dirtying marker: every paper
  // node carries the ids of the active projects it belongs to, so a membership
  // change IS graph data. Asserting it stayed fresh encoded the opposite.
  assert.ok(PROJECT_MEMBERSHIP_QUERY_KEYS.includes(GRAPH_QUERY_KEY));
});

// --- Graph staleness -------------------------------------------------------

/** Runs `fn` with a listener attached and reports how many times it fired. */
async function graphDirtyCount(fn: (qc: QueryClient) => Promise<void>): Promise<number> {
  let fired = 0;
  const off = onGraphDirtying(() => {
    fired++;
  });
  try {
    await fn(new QueryClient());
  } finally {
    off();
  }
  return fired;
}

// The defect: GraphPage flagged its Refresh button from query-cache
// `invalidate` events, which react-query emits only for queries that are
// actually cached. An author merge from /authors invalidates ["authors"] and
// ["author", id]; if nothing holds either — the graph page keeps only
// ["stats"] and a picker's ["projects"] alive — the event never fires and the
// canvas keeps drawing the merged-away author with no cue that it is stale.
test("every graph-changing operation announces itself with nothing cached", async () => {
  for (const invalidate of [
    invalidatePaperQueries,
    invalidatePaperMutationQueries,
    invalidateProjectMutationQueries,
    invalidateProjectMembershipQueries,
    invalidateAuthorQueries,
  ]) {
    assert.equal(await graphDirtyCount(invalidate), 1, `${invalidate.name} announced nothing`);
  }
});

test("note and annotation edits leave the graph alone", async () => {
  assert.equal(await graphDirtyCount(invalidateNoteQueries), 0);
  assert.equal(await graphDirtyCount(invalidateAnnotationQueries), 0);
  for (const keys of [NOTE_QUERY_KEYS, ANNOTATION_QUERY_KEYS]) {
    assert.ok(!keys.includes(GRAPH_QUERY_KEY), "a note/annotation set claims the graph");
  }
});

test("unsubscribing stops the announcements", async () => {
  let fired = 0;
  const off = onGraphDirtying(() => {
    fired++;
  });
  await invalidatePaperQueries(new QueryClient());
  off();
  await invalidatePaperQueries(new QueryClient());
  assert.equal(fired, 1);
});

// Author rename / delete / merge had no registry owner: each of AuthorPage's
// three mutations spelled out its own key list, and they disagreed — the
// delete refreshed only ["authors"], leaving the ["author", id] the page
// itself was rendering fresh.
test("author mutations share one key set covering all three call sites", async () => {
  const stale = await staleAfter(
    [["authors"], ["author", 7], ["author-merge-candidates", 7], ["settings"]],
    invalidateAuthorQueries
  );

  assert.ok(stale.has(JSON.stringify(["authors"])));
  assert.ok(stale.has(JSON.stringify(["author", 7])));
  assert.ok(stale.has(JSON.stringify(["author-merge-candidates", 7])));
  assert.ok(!stale.has(JSON.stringify(["settings"])));
  assert.ok(AUTHOR_MUTATION_QUERY_KEYS.includes(GRAPH_QUERY_KEY));
});

test("purging papers drops their persisted reading status", () => {
  const removed: string[] = [];
  const fake: ReadingStatusRemover = { remove: (id) => removed.push(id) };

  forgetPurgedPapers(["arxiv:1", "arxiv:2"], fake);

  assert.deepEqual(removed, ["arxiv:1", "arxiv:2"]);
});

test("partial-failure message reports the failed count against the total", () => {
  assert.equal(partialFailureMessage(2, 5), "2 of 5 papers could not be added");
  assert.equal(partialFailureMessage(1, 1), "1 of 1 paper could not be added");
});

/** Records every ProjectPickerActions call for asserting against. */
function fakePicker() {
  const calls = {
    errors: [] as (string | null)[],
    selected: [] as string[][],
    done: 0,
    namesCleared: 0,
  };
  const ui: ProjectPickerActions = {
    setError: (m) => calls.errors.push(m),
    selectFailures: (ids) => calls.selected.push(ids),
    onDone: () => calls.done++,
    clearName: () => calls.namesCleared++,
  };
  return { ui, calls };
}

// The defect: Library threw on partial failure while Graph re-selected the
// failures. One contract now: report + re-select, never throw.
test("add-to-project partial failure re-selects failures and stays open", () => {
  const { ui, calls } = fakePicker();
  const opts = addToProjectMutationOptions(new QueryClient(), ui);

  opts.onSuccess?.(["arxiv:2"], { projectId: 1, sourceIds: ["arxiv:1", "arxiv:2"] }, undefined);

  assert.deepEqual(calls.selected, [["arxiv:2"]]);
  assert.deepEqual(calls.errors, ["1 of 2 papers could not be added"]);
  assert.equal(calls.done, 0);
});

test("add-to-project full success closes the picker", () => {
  const { ui, calls } = fakePicker();
  const opts = addToProjectMutationOptions(new QueryClient(), ui);

  opts.onSuccess?.([], { projectId: 1, sourceIds: ["arxiv:1"] }, undefined);

  assert.equal(calls.done, 1);
  assert.deepEqual(calls.selected, []);
  assert.deepEqual(calls.errors, [null]);
});

test("create-project clears the name even on partial failure", () => {
  const { ui, calls } = fakePicker();
  const opts = createProjectMutationOptions(new QueryClient(), ui);

  opts.onSuccess?.(["arxiv:1"], { name: "p", sourceIds: ["arxiv:1"] }, undefined);

  assert.equal(calls.namesCleared, 1);
  assert.deepEqual(calls.selected, [["arxiv:1"]]);
  assert.deepEqual(calls.errors, ["Project created, but 1 paper could not be added"]);
  assert.equal(calls.done, 0);

  opts.onSuccess?.([], { name: "p", sourceIds: ["arxiv:1"] }, undefined);
  assert.equal(calls.namesCleared, 2);
  assert.equal(calls.done, 1);
});

test("mutation errors surface as picker messages", () => {
  const { ui, calls } = fakePicker();
  const add = addToProjectMutationOptions(new QueryClient(), ui);
  const create = createProjectMutationOptions(new QueryClient(), ui);

  add.onError?.(new Error("boom"), { projectId: 1, sourceIds: [] }, undefined);
  create.onError?.(new Error("bang"), { name: "p", sourceIds: [] }, undefined);

  assert.deepEqual(calls.errors, ["boom", "bang"]);
});
