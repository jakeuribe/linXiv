// Run: node --experimental-strip-types --test src/lib/pdfAnchor.rects.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { coalesceRectsIntoLines } from "./pdfAnchor.ts";

test("per-word rects on one line collapse to a single spanning rect", () => {
  // Three words with gaps between them, all on the same visual line.
  const lines = coalesceRectsIntoLines([
    { left: 10, top: 100, right: 40, bottom: 112 },
    { left: 48, top: 100, right: 70, bottom: 112 },
    { left: 78, top: 101, right: 120, bottom: 113 }, // slight top jitter
  ]);
  assert.equal(lines.length, 1);
  assert.deepEqual(lines[0], { left: 10, top: 100, right: 120, bottom: 113 });
});

test("two lines yield two rects", () => {
  const lines = coalesceRectsIntoLines([
    { left: 10, top: 100, right: 40, bottom: 112 },
    { left: 48, top: 100, right: 90, bottom: 112 },
    { left: 10, top: 120, right: 55, bottom: 132 },
    { left: 60, top: 120, right: 88, bottom: 132 },
  ]);
  assert.equal(lines.length, 2);
  assert.deepEqual(lines[0], { left: 10, top: 100, right: 90, bottom: 112 });
  assert.deepEqual(lines[1], { left: 10, top: 120, right: 88, bottom: 132 });
});

test("empty input yields no rects", () => {
  assert.deepEqual(coalesceRectsIntoLines([]), []);
});
