import { test } from "node:test";
import assert from "node:assert/strict";
import { diffSummary, formatActor, formatTime } from "./history.ts";
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

test("formatActor: mine wins over the hex prefix", () => {
  assert.equal(formatActor("deadbeefcafe", true), "This device");
  assert.equal(formatActor("deadbeefcafe0123", false), "deadbeef");
});

test("formatTime: zero renders as em dash", () => {
  assert.equal(formatTime(0), "—");
  assert.notEqual(formatTime(1_700_000_000), "—");
});
