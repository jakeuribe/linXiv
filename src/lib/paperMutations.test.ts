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
  type ReadingStatusRemover,
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
