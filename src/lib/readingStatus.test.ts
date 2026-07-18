// Run: node --experimental-strip-types --test src/lib/readingStatus.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  cycleStatus,
  isReadingListProject,
  queueOf,
  statusLabel,
} from "./readingStatus.ts";

test("cycleStatus cycles unread → reading → read → unread", () => {
  assert.equal(cycleStatus(undefined), "reading");
  assert.equal(cycleStatus("reading"), "read");
  assert.equal(cycleStatus("read"), undefined);
});

test("statusLabel returns human-readable status", () => {
  assert.equal(statusLabel(undefined), "Unread");
  assert.equal(statusLabel("reading"), "Reading");
  assert.equal(statusLabel("read"), "Read");
});

test("isReadingListProject identifies projects tagged reading-list", () => {
  assert.equal(isReadingListProject({ project_tags: ["reading-list"] }), true);
  assert.equal(isReadingListProject({ project_tags: ["other"] }), false);
  assert.equal(isReadingListProject({ project_tags: [] }), false);
});

test("isReadingListProject recognizes reading-list tag regardless of casing", () => {
  assert.equal(isReadingListProject({ project_tags: ["Reading-List"] }), true);
  assert.equal(isReadingListProject({ project_tags: ["READING-LIST"] }), true);
  assert.equal(isReadingListProject({ project_tags: ["reading-LIST"] }), true);
});

test("queueOf derives listed, unread-first papers", () => {
  const papers = [
    { source_id: "a" },
    { source_id: "b" },
    { source_id: "c" },
    { source_id: "d" },
  ];
  const q = queueOf(papers, new Set(["a", "b", "c"]), {
    a: "read",
    c: "reading",
  });
  assert.deepEqual(
    q.map((p) => p.source_id),
    ["c", "b"]
  );
  assert.deepEqual(queueOf(papers, new Set(), {}), []);
});
