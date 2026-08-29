// Run: node --experimental-transform-types --test src/lib/graph/tooltip.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";

import { indexView } from "./model.ts";
import { SUMMARY_MAX, tooltipFor, truncate } from "./tooltip.ts";
import { paper, sampleView } from "./fixture.ts";

const view = sampleView();
const index = indexView(view);
/** Nothing held back: every paper is drawn. */
const allPapers = new Set(view.papers.map((p) => p.id));

test("truncate cuts on a word boundary and marks the cut", () => {
  assert.equal(truncate("short", 20), "short");
  assert.equal(truncate("  collapses\n  whitespace  ", 40), "collapses whitespace");
  assert.equal(truncate("alpha beta gamma delta", 12), "alpha beta…");
  // A single long token has no boundary worth honouring, so it is cut hard.
  assert.equal(truncate("a".repeat(30), 10), `${"a".repeat(10)}…`);
});

test("a paper's head line reads category · date · PDF", () => {
  const t = tooltipFor("1", "paper", index, allPapers);
  assert.equal(t.title, "Attention Is All You Need");
  assert.equal(t.lines[0], "cs.LG · 2024-01-01 · PDF");
});

// `published` is already null for the "no date" sentinel, so this is sayable
// rather than a bogus year 1.
test("an undated paper says so instead of showing year 1", () => {
  const t = tooltipFor("2", "paper", index, allPapers);
  assert.equal(t.lines[0], "cs.CL · No publication date · No PDF");
});

test("a paper lists the tag chips the canvas drew for it", () => {
  // The backend already deduped and resolved the spelling, so this line names
  // exactly what is on the canvas rather than the raw metadata column.
  assert.equal(tooltipFor("1", "paper", index, allPapers).lines[1], "ML · nlp");
});

test("a paper's abstract is truncated rather than dumped", () => {
  const long = sampleView();
  long.papers[0] = paper({ ...long.papers[0], summary: "word ".repeat(200) });
  const t = tooltipFor("1", "paper", indexView(long), allPapers);
  const summary = t.lines[t.lines.length - 1];
  assert.ok(summary.length <= SUMMARY_MAX + 1, `${summary.length} chars`);
  assert.ok(summary.endsWith("…"));
});

test("a paper with no category, tags or abstract shows just the head line", () => {
  const bare = sampleView();
  bare.papers = [paper({ id: "1", label: "Bare" })];
  bare.edges = [];
  const t = tooltipFor("1", "paper", indexView(bare), new Set(["1"]));
  assert.deepEqual(t.lines, ["No publication date · No PDF"]);
});

// The degree is a fact about the library — the Authors page reports the same
// number — so filtering the canvas must not silently rewrite it.
test("an author reports its degree, unqualified when it is all drawn", () => {
  assert.deepEqual(tooltipFor("author::7", "author", index, allPapers).lines, [
    "Author · 2 papers",
  ]);
  assert.deepEqual(tooltipFor("author::8", "author", index, allPapers).lines, [
    "Author · 1 paper",
  ]);
});

test("a filtered canvas reports both the degree and what is left of it", () => {
  assert.deepEqual(tooltipFor("author::7", "author", index, new Set(["1"])).lines, [
    "Author · 2 papers (1 shown)",
  ]);
  assert.deepEqual(tooltipFor("author::7", "author", index, new Set()).lines, [
    "Author · 2 papers (none shown)",
  ]);
});

test("a tag reports its degree the same way", () => {
  assert.deepEqual(tooltipFor("tag::nlp", "tag", index, allPapers).lines, ["Tag · 2 papers"]);
  assert.deepEqual(tooltipFor("tag::nlp", "tag", index, new Set(["2"])).lines, [
    "Tag · 2 papers (1 shown)",
  ]);
});
