// Run: node --experimental-transform-types --test src/lib/graph/layout.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";

import {
  LAYOUT_SEED_KEY,
  SEED_JITTER,
  SEED_SPREAD,
  layoutRng,
  mulberry32,
  randomizePositions,
  seedPositions,
} from "./layout.ts";

/** A deterministic stand-in for Math.random, so a seeded position is exact. */
const fixedRng = (value: number) => () => value;

test("mulberry32 is deterministic and stays in [0, 1)", () => {
  const a = mulberry32(42);
  const b = mulberry32(42);
  const drawn = Array.from({ length: 8 }, () => a());
  assert.deepEqual(drawn, Array.from({ length: 8 }, () => b()));
  for (const v of drawn) {
    assert.ok(v >= 0 && v < 1, `${v} out of range`);
  }
  assert.notDeepEqual(drawn, Array.from({ length: 8 }, mulberry32(43)));
});

// Absent the key — the default for every real user — nothing changes.
test("layoutRng falls back to Math.random with no seed stored", () => {
  assert.equal(layoutRng({ getItem: () => null }), Math.random);
  assert.equal(layoutRng({ getItem: () => "" }), Math.random);
});

test("layoutRng seeds a reproducible sequence when the key is set", () => {
  const store = { getItem: (k: string) => (k === LAYOUT_SEED_KEY ? "7" : null) };
  assert.deepEqual(
    Array.from({ length: 4 }, layoutRng(store)),
    Array.from({ length: 4 }, mulberry32(7))
  );
});

test("a cold load seeds every node inside the spread box", () => {
  const nodes = seedPositions(["1", "2"], [], new Map(), fixedRng(1));
  // rand() - 0.5 = 0.5, so each coordinate lands on the box's far edge.
  assert.deepEqual(nodes, [
    { id: "1", x: SEED_SPREAD / 2, y: SEED_SPREAD / 2 },
    { id: "2", x: SEED_SPREAD / 2, y: SEED_SPREAD / 2 },
  ]);
});

test("a surviving node keeps the position the settled layout left it at", () => {
  const previous = new Map([["1", { x: 120, y: -40 }]]);
  const [kept] = seedPositions(["1"], [], previous, fixedRng(0));
  assert.deepEqual(kept, { id: "1", x: 120, y: -40 });
});

// The cold-load answer — a random point in an 800x800 box at the origin — is the
// wrong one for a node arriving into a settled layout: the force layout spreads
// the graph far wider than that box, so a paper imported elsewhere in the app
// would appear nowhere near the authors and tags it is joined to.
test("a new node is seeded at the centroid of its placed neighbours", () => {
  const previous = new Map([
    ["author::7", { x: 100, y: 0 }],
    ["author::8", { x: 300, y: 40 }],
  ]);
  const edges = [
    { source: "9", target: "author::7" },
    { source: "9", target: "author::8" },
  ];
  const nodes = seedPositions(["9", "author::7", "author::8"], edges, previous, fixedRng(0.5));
  const fresh = nodes.find((n) => n.id === "9")!;
  // rand() - 0.5 = 0, so the jitter contributes nothing and the centroid is exact.
  assert.deepEqual(fresh, { id: "9", x: 200, y: 20 });
});

test("the jitter keeps co-seeded nodes off the exact same point", () => {
  const previous = new Map([["author::7", { x: 0, y: 0 }]]);
  const edges = [{ source: "9", target: "author::7" }];
  const nodes = seedPositions(["9", "author::7"], edges, previous, fixedRng(1));
  const fresh = nodes.find((n) => n.id === "9")!;
  assert.deepEqual(fresh, { id: "9", x: SEED_JITTER / 2, y: SEED_JITTER / 2 });
});

// Two passes is what an imported paper needs: the first puts the PAPER beside
// the authors and tags it shares with the library, the second puts its brand-new
// author nodes beside the paper.
test("a second pass reaches a new node that only touches another new one", () => {
  const previous = new Map([["tag::ml", { x: 60, y: 60 }]]);
  const edges = [
    { source: "9", target: "tag::ml" }, // pass 1 places the paper
    { source: "9", target: "author::99" }, // pass 2 places its new author
  ];
  const nodes = seedPositions(["9", "author::99", "tag::ml"], edges, previous, fixedRng(0.5));
  assert.deepEqual(nodes.find((n) => n.id === "9"), { id: "9", x: 60, y: 60 });
  assert.deepEqual(nodes.find((n) => n.id === "author::99"), { id: "author::99", x: 60, y: 60 });
});

// A node placed in this pass must not seed another one in the same pass, or the
// result would depend on the order the payload happened to list nodes in.
test("seeding within one pass does not cascade", () => {
  const previous = new Map([["a", { x: 0, y: 0 }]]);
  // b touches a (pass 1); c touches only b, so it must wait for pass 2.
  const edges = [
    { source: "b", target: "a" },
    { source: "c", target: "b" },
  ];
  const forward = seedPositions(["a", "b", "c"], edges, previous, fixedRng(0.5));
  const reversed = seedPositions(["c", "b", "a"], edges, previous, fixedRng(0.5));
  const at = (nodes: typeof forward, id: string) => {
    const n = nodes.find((x) => x.id === id)!;
    return { x: n.x, y: n.y };
  };
  assert.deepEqual(at(forward, "c"), at(reversed, "c"));
});

// An island with nothing to sit by keeps its box seed, which is the honest
// answer rather than a centroid of nothing.
test("an unreachable new node keeps its random box seed", () => {
  const previous = new Map([["a", { x: 500, y: 500 }]]);
  const nodes = seedPositions(["a", "island"], [], previous, fixedRng(1));
  assert.deepEqual(nodes.find((n) => n.id === "island"), {
    id: "island",
    x: SEED_SPREAD / 2,
    y: SEED_SPREAD / 2,
  });
});

test("randomize throws every position back into the seed box", () => {
  const nodes = [
    { id: "a", x: 9000, y: -9000 },
    { id: "b", x: 1, y: 2 },
  ];
  randomizePositions(nodes, fixedRng(0));
  assert.deepEqual(nodes, [
    { id: "a", x: -SEED_SPREAD / 2, y: -SEED_SPREAD / 2 },
    { id: "b", x: -SEED_SPREAD / 2, y: -SEED_SPREAD / 2 },
  ]);
});
