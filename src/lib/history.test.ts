import { test } from "node:test";
import assert from "node:assert/strict";
import {
  diffSummary,
  formatActor,
  formatTime,
  isMineChange,
  viewerIdentities,
  wordDiff,
} from "./history.ts";
import type { HistoryDiff } from "../types/api";

const empty: HistoryDiff = {
  papers_added: [],
  papers_removed: [],
  tags_added: [],
  tags_removed: [],
  notes_added: [],
  notes_removed: [],
  notes_changed: [],
  annotations_added: [],
  annotations_removed: [],
  annotations_changed: [],
  meta: [],
};

test("diffSummary: empty diff is an empty string", () => {
  assert.equal(diffSummary(empty), "");
});

test("diffSummary: counts with signs, pluralized, meta by field name", () => {
  const d: HistoryDiff = {
    ...empty,
    papers_added: [
      { source_id: "arxiv:1", title: "A" },
      { source_id: "arxiv:2", title: "B" },
    ],
    notes_removed: [{ uuid: "u", title: "n", from: "x", to: null }],
    annotations_changed: [{ uuid: "v", title: "arxiv:1", from: "a", to: "b" }],
    meta: [{ field: "name", from: "Old", to: "New" }],
  };
  assert.equal(
    diffSummary(d),
    "+2 papers · −1 note · ~1 annotation · ~name"
  );
});

test("formatActor: mine > display name > hex prefix", () => {
  assert.equal(formatActor("deadbeefcafe", true), "This device");
  assert.equal(formatActor("deadbeefcafe", true, "Ada"), "This device");
  assert.equal(formatActor("deadbeefcafe0123", false), "deadbeef");
  assert.equal(formatActor("deadbeefcafe0123", false, "Ada"), "Ada");
  assert.equal(formatActor("deadbeefcafe0123", false, ""), "deadbeef");
  assert.equal(formatActor("deadbeefcafe0123", false, null), "deadbeef");
});

test("wordDiff: identical strings are a single same run", () => {
  assert.deepEqual(wordDiff("a b c", "a b c"), [
    { kind: "same", text: "a b c" },
  ]);
});

test("wordDiff: disjoint strings are del then add", () => {
  assert.deepEqual(wordDiff("oldtext", "newwords"), [
    { kind: "del", text: "oldtext" },
    { kind: "add", text: "newwords" },
  ]);
});

test("wordDiff: middle-word edit is same/del/add/same, lossless", () => {
  const runs = wordDiff("the quick fox", "the slow fox");
  assert.deepEqual(runs, [
    { kind: "same", text: "the " },
    { kind: "del", text: "quick" },
    { kind: "add", text: "slow" },
    { kind: "same", text: " fox" },
  ]);
  const joined = (kinds: string[]) =>
    runs.filter((r) => kinds.includes(r.kind)).map((r) => r.text).join("");
  assert.equal(joined(["same", "del"]), "the quick fox");
  assert.equal(joined(["same", "add"]), "the slow fox");
});

test("wordDiff: clip-size prose stays word-level; degenerate walls bail", () => {
  // ~2000 chars of ordinary words — must produce a real word-level diff.
  const words = Array.from({ length: 400 }, (_, i) => `word${i}`);
  const from = words.join(" ");
  const to = words.map((w, i) => (i === 200 ? "EDITED" : w)).join(" ");
  const runs = wordDiff(from, to);
  assert.ok(runs.some((r) => r.kind === "same" && r.text.length > 100));
  assert.deepEqual(
    runs.filter((r) => r.kind !== "same"),
    [
      { kind: "del", text: "word200" },
      { kind: "add", text: "EDITED" },
    ]
  );
  // A wall of single-char tokens overflows the cell cap → del/add fallback.
  const wall = Array.from({ length: 1200 }, () => "a").join(" ");
  assert.deepEqual(wordDiff(wall, `${wall} b`), [
    { kind: "del", text: wall },
    { kind: "add", text: `${wall} b` },
  ]);
});

test("wordDiff: empty sides", () => {
  assert.deepEqual(wordDiff("", "hi"), [{ kind: "add", text: "hi" }]);
  assert.deepEqual(wordDiff("hi", ""), [{ kind: "del", text: "hi" }]);
  assert.deepEqual(wordDiff("", ""), []);
});

test("isMineChange: viewer identities win over the wire flag, any casing", () => {
  const ids = viewerIdentities("DEADBEEF", null, undefined, "AA".repeat(32));
  assert.deepEqual(ids, ["deadbeef", "aa".repeat(32)]);
  // The serving node's wire flag is overridden in both directions.
  assert.equal(isMineChange("DeadBeef", false, ids), true);
  assert.equal(isMineChange("cafe0123", true, ids), false);
  // No local identities known: fall back to the wire flag.
  assert.equal(isMineChange("cafe0123", true, []), true);
  assert.equal(isMineChange("cafe0123", false, []), false);
});

test("formatTime: zero renders as em dash", () => {
  assert.equal(formatTime(0), "—");
  assert.notEqual(formatTime(1_700_000_000), "—");
});
