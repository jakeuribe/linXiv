// Run: node --experimental-transform-types --test src/lib/graph/filter.test.ts
//
// The filter is the one piece of the Knowledge Graph that stays on the client
// (the rest of its derivations live in Rust): an excluded paper is still DRAWN
// — as an 8% ghost — so "matched" is a rendering state, not a WHERE clause.
// It is a pure function, so these run against it directly.
import { test } from "node:test";
import assert from "node:assert/strict";

import { indexView } from "./model.ts";
import type { GraphFilterState } from "./filter.ts";
import {
  EMPTY_FILTER,
  joinTypes,
  noMatchCause,
  activeFilterSummary,
  activeTagFilterSummary,
  evalTagRows,
  layoutIds,
  matchGraph,
  projectsMatchingName,
  projectsWithTag,
} from "./filter.ts";
import { paper, project, sampleView } from "./fixture.ts";

function run(over: Partial<GraphFilterState> = {}, view = sampleView()) {
  return matchGraph(view, indexView(view), { ...EMPTY_FILTER, ...over });
}

const ids = (s: Set<string>) => [...s].sort();

test("no filter matches every node of every type", () => {
  const m = run();
  assert.deepEqual(ids(m.papers), ["1", "2"]);
  assert.deepEqual(ids(m.authors), ["author::7", "author::8"]);
  assert.deepEqual(ids(m.tags), ["tag::ml", "tag::nlp"]);
  assert.equal(m.drawnCount, 6);
});

test("category matches by case-insensitive substring", () => {
  assert.deepEqual(ids(run({ category: "cs.lg" }).papers), ["1"]);
  assert.deepEqual(ids(run({ category: "cs." }).papers), ["1", "2"]);
  assert.deepEqual(ids(run({ category: "physics" }).papers), []);
});

test("has-PDF keeps only papers with a local file", () => {
  assert.deepEqual(ids(run({ hasPdf: true }).papers), ["1"]);
});

test("title matches by case-insensitive substring", () => {
  assert.deepEqual(ids(run({ title: "ATTENTION" }).papers), ["1"]);
});

// The Author box matches `GraphPaper.author_keys`, which the backend builds from
// the PAPER_TO_AUTHOR links — the client used to rebuild it by walking the edge
// list and lowercasing every author node's label on every load.
test("author matches by substring across a paper's own authors", () => {
  assert.deepEqual(ids(run({ author: "lovelace" }).papers), ["1", "2"]);
  assert.deepEqual(ids(run({ author: "turing" }).papers), ["1"]);
  assert.deepEqual(ids(run({ author: "hinton" }).papers), []);
});

// The sentinel is folded to null server-side, so an undated paper is not
// filtered BY date. Forwarding `0001-01-01` raw made it read as a real date in
// year 1, and every `From` filter silently dropped every undated paper.
test("a date range never drops an undated paper", () => {
  assert.deepEqual(ids(run({ dateFrom: "2020-01-01" }).papers), ["1", "2"]);
  assert.deepEqual(ids(run({ dateFrom: "2024-06-01" }).papers), ["2"]);
  assert.deepEqual(ids(run({ dateTo: "2023-12-31" }).papers), ["2"]);
});

test("authors and tags are matched by adjacency to a matching paper", () => {
  const m = run({ title: "Other" });
  assert.deepEqual(ids(m.papers), ["2"]);
  // Alan Turing is only on paper 1, so he drops out with it; the shared author
  // and the shared tag stay.
  assert.deepEqual(ids(m.authors), ["author::7"]);
  assert.deepEqual(ids(m.tags), ["tag::nlp"]);
});

// Folding the Visibility checkboxes into the match is what made unchecking
// "Papers" blank the entire canvas: it emptied the paper set, and author/tag
// visibility is derived purely from edges to matching papers.
test("a Visibility checkbox hides a type without changing what matched", () => {
  const m = run({ showPapers: false });
  assert.deepEqual(ids(m.papers), ["1", "2"], "the papers still MATCH");
  assert.deepEqual(ids(m.authors), ["author::7", "author::8"]);
  assert.deepEqual([...m.hiddenTypes], ["paper"]);
  // …but they are not drawn, so they do not count towards "is anything visible".
  assert.equal(m.drawnCount, 4);
});

test("all three types hidden draws nothing at all", () => {
  const m = run({ showPapers: false, showAuthors: false, showTags: false });
  assert.equal(m.hiddenTypes.size, 3);
  assert.equal(m.drawnCount, 0);
});

// The layout membership is what the canvas hands the charge, collision, link
// and pinning passes. It differs from the MATCH in exactly one way: a type a
// Visibility checkbox switched off is out of the layout even though it still
// matches — an invisible node must not shape the layout of the visible ones.
test("with nothing hidden the layout runs over every matched node", () => {
  const m = run();
  assert.deepEqual(
    [...layoutIds(m)].sort(),
    ["1", "2", "author::7", "author::8", "tag::ml", "tag::nlp"]
  );
});

test("a hidden type is out of the layout even though it still matches", () => {
  const m = run({ showAuthors: false });
  assert.deepEqual(ids(m.authors), ["author::7", "author::8"], "still MATCHED…");
  const layout = layoutIds(m);
  assert.ok(!layout.has("author::7") && !layout.has("author::8"), "…but not laid out");
  assert.deepEqual([...layout].sort(), ["1", "2", "tag::ml", "tag::nlp"]);
});

test("hiding Papers leaves only authors and tags in the layout", () => {
  // Every edge has a paper endpoint, so this is the membership that empties the
  // link set — the canvas must be handed that honestly, not papered over here.
  const m = run({ showPapers: false });
  assert.deepEqual(
    [...layoutIds(m)].sort(),
    ["author::7", "author::8", "tag::ml", "tag::nlp"]
  );
});

// "Show highlighted only" changes opacity, never membership: the non-matching
// nodes were already out of the layout, and the matching ones must not move.
test("isolate does not change the layout membership", () => {
  const m = run({ title: "Other" });
  assert.deepEqual([...layoutIds(m)].sort(), [...layoutIds(run({ title: "Other", isolate: true }))].sort());
});

test("an attribute filter and a hidden type compose in the layout", () => {
  const m = run({ title: "Other", showTags: false });
  // Paper 2 matches, its author matches, its tag matches but the type is hidden.
  assert.deepEqual([...layoutIds(m)].sort(), ["2", "author::7"]);
});

test("tag rows fold left through their AND/OR toggles", () => {
  assert.equal(evalTagRows(["ml", "nlp"], []), true, "no rows matches everything");
  assert.equal(evalTagRows(["ml"], [{ op: "AND", tag: "ML" }]), true, "rows fold case");
  assert.equal(evalTagRows(["ml"], [{ op: "AND", tag: " ml " }]), true, "…and whitespace");
  assert.equal(
    evalTagRows(["ml"], [{ op: "AND", tag: "ml" }, { op: "AND", tag: "nlp" }]),
    false
  );
  assert.equal(
    evalTagRows(["ml"], [{ op: "AND", tag: "ml" }, { op: "OR", tag: "nlp" }]),
    true
  );
  // Left fold: (missing OR ml) AND nlp.
  assert.equal(
    evalTagRows(["ml"], [
      { op: "AND", tag: "missing" },
      { op: "OR", tag: "ml" },
      { op: "AND", tag: "nlp" },
    ]),
    false
  );
});

test("a tag row filters papers by their own tags", () => {
  assert.deepEqual(ids(run({ tagRows: [{ op: "AND", tag: "ML" }] }).papers), ["1"]);
  assert.deepEqual(ids(run({ tagRows: [{ op: "AND", tag: "nlp" }] }).papers), ["1", "2"]);
});

test("a project row matches by substring on the project name", () => {
  assert.deepEqual(ids(run({ projectNames: ["transform"] }).papers), ["1"]);
});

// The rows are free text, so a typo — or a project renamed or deleted since the
// row was added — resolves to no project at all. That has to match NO paper; an
// empty id list read as "no filter" would silently show everything.
test("a project row that resolves to nothing matches no paper", () => {
  assert.deepEqual(ids(run({ projectNames: ["nope"] }).papers), []);
});

test("a project tag row matches the whole tag, case-insensitively", () => {
  assert.deepEqual(ids(run({ projectTags: ["READING"] }).papers), ["1"]);
  // Whole, never by substring — the backend compares project tags that way too.
  assert.deepEqual(ids(run({ projectTags: ["read"] }).papers), []);
});

test("project rows resolve against the payload's own project list", () => {
  const view = sampleView();
  view.projects = [
    project({ id: 10, name: "Transformers", tags: ["reading"] }),
    project({ id: 11, name: "Transformer Ablations", tags: ["reading"] }),
    project({ id: 12, name: "Diffusion", tags: ["later"] }),
  ];
  assert.deepEqual(
    projectsMatchingName(view, "transformer").map((p) => p.id),
    [10, 11]
  );
  assert.deepEqual(
    projectsWithTag(view, "Reading").map((p) => p.id),
    [10, 11]
  );
});

test("filters compose — every clause has to hold", () => {
  assert.deepEqual(ids(run({ category: "cs.LG", author: "lovelace" }).papers), ["1"]);
  assert.deepEqual(ids(run({ category: "cs.CL", author: "turing" }).papers), []);
});

test("a paper with no tags survives a filter that names none", () => {
  const view = sampleView();
  view.papers.push(paper({ id: "3", label: "Untagged" }));
  view.categories = ["cs.CL", "cs.LG"];
  assert.ok(matchGraph(view, indexView(view), EMPTY_FILTER).papers.has("3"));
});

test("the Filters badge names every control that is on, in panel order", () => {
  assert.deepEqual(activeFilterSummary(EMPTY_FILTER), []);
  assert.deepEqual(
    activeFilterSummary({
      ...EMPTY_FILTER,
      showAuthors: false,
      category: " cs.LG ",
      hasPdf: true,
      dateFrom: "2024-01-01",
      title: "attention",
      isolate: true,
    }),
    [
      "Authors hidden",
      "Category: cs.LG",
      "Has PDF only",
      "Published from 2024-01-01",
      "Title: attention",
      "Show highlighted only",
    ]
  );
});

// Text left sitting in an add-box is not a filter — nothing reads it until
// "+"/Enter — which is the same line matchGraph draws.
test("the Tag Filter badge counts rows and spells out their AND/OR", () => {
  assert.deepEqual(activeTagFilterSummary(EMPTY_FILTER), []);
  assert.deepEqual(
    activeTagFilterSummary({
      ...EMPTY_FILTER,
      projectNames: ["Transformers"],
      projectTags: ["reading"],
      tagRows: [
        { op: "AND", tag: "ml" },
        { op: "OR", tag: "nlp" },
      ],
    }),
    ["Project: Transformers", "Project tag: reading", "Tag: ml", "OR Tag: nlp"]
  );
});

// The notice has to name the panel that actually emptied the canvas. Counting
// hidden TYPES got this wrong on the commonest library there is — one with no
// tags — where switching off Papers and Authors empties it with two boxes
// unchecked rather than three.
test("an empty canvas with papers still matching blames Visibility, not the filters", () => {
  const m = run({ showPapers: false, showAuthors: false, showTags: false });
  assert.equal(m.drawnCount, 0);
  assert.deepEqual(noMatchCause(m), { kind: "visibility", types: ["Papers", "Authors", "Tags"] });
});

test("a library with no tags blames only the types that had something to show", () => {
  const view = sampleView();
  view.tags = [];
  view.edges = view.edges.filter((e) => !e.target.startsWith("tag::"));
  for (const p of view.papers) {
    p.tags = [];
    p.tag_keys = [];
  }
  const m = matchGraph(view, indexView(view), {
    ...EMPTY_FILTER,
    showPapers: false,
    showAuthors: false,
  });
  assert.equal(m.drawnCount, 0, "nothing is drawn…");
  assert.equal(m.hiddenTypes.size, 2, "…with only two boxes unchecked");
  // "Tags" must not be named: this library has none to hide.
  assert.deepEqual(noMatchCause(m), { kind: "visibility", types: ["Papers", "Authors"] });
});

test("an empty canvas with no paper matching blames the attribute filters", () => {
  const m = run({ title: "zzzznothing" });
  assert.equal(m.drawnCount, 0);
  assert.deepEqual(noMatchCause(m), { kind: "filters" });
});

test("joinTypes reads as a sentence", () => {
  assert.equal(joinTypes([]), "Every node type");
  assert.equal(joinTypes(["Papers"]), "Papers");
  assert.equal(joinTypes(["Papers", "Authors"]), "Papers and Authors");
  assert.equal(joinTypes(["Papers", "Authors", "Tags"]), "Papers, Authors and Tags");
});
