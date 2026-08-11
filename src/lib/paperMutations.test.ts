// Run: node --experimental-transform-types --test src/lib/paperMutations.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { QueryClient } from "@tanstack/react-query";
import {
  PAPER_QUERY_KEYS,
  invalidatePaperQueries,
  invalidateProjectMembershipQueries,
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

test("project membership invalidation stays narrow", async () => {
  const stale = await staleAfter(
    [["projects"], ["project", "3"], ["graph"], ["stats"]],
    invalidateProjectMembershipQueries
  );

  assert.ok(stale.has(JSON.stringify(["projects"])));
  assert.ok(stale.has(JSON.stringify(["project", "3"])));
  assert.ok(!stale.has(JSON.stringify(["graph"])));
  assert.ok(!stale.has(JSON.stringify(["stats"])));
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
