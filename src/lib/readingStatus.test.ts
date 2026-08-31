// Run: node --experimental-strip-types --test src/lib/readingStatus.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  cycleStatus,
  isReadingListProject,
  parsePersistedReadingStatuses,
  pushLegacyStatuses,
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

test("parsePersistedReadingStatuses salvages valid entries from the legacy blob", () => {
  const raw = JSON.stringify({
    state: { statuses: { a: "reading", b: "read", c: "bogus", d: 3 } },
    version: 1,
  });
  assert.deepEqual(parsePersistedReadingStatuses(raw), {
    a: "reading",
    b: "read",
  });
});

test("parsePersistedReadingStatuses yields {} on any garbage shape", () => {
  assert.deepEqual(parsePersistedReadingStatuses(null), {});
  assert.deepEqual(parsePersistedReadingStatuses("not json"), {});
  assert.deepEqual(parsePersistedReadingStatuses("null"), {});
  assert.deepEqual(parsePersistedReadingStatuses('{"state":{}}'), {});
  assert.deepEqual(parsePersistedReadingStatuses('{"state":{"statuses":7}}'), {});
});

test("pushLegacyStatuses pushes every entry and reports success", async () => {
  const pushed: [string, string][] = [];
  const ok = await pushLegacyStatuses({ a: "reading", b: "read" }, async (sid, s) => {
    pushed.push([sid, s]);
  });
  assert.equal(ok, true);
  assert.deepEqual(pushed, [
    ["a", "reading"],
    ["b", "read"],
  ]);
});

test("pushLegacyStatuses keeps going past a failure but reports it", async () => {
  const pushed: string[] = [];
  const ok = await pushLegacyStatuses({ a: "reading", b: "read" }, async (sid) => {
    if (sid === "a") throw new Error("backend down");
    pushed.push(sid);
  });
  assert.equal(ok, false);
  assert.deepEqual(pushed, ["b"]);
});
