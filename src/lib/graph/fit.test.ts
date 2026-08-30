// Run: node --experimental-transform-types --test src/lib/graph/fit.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";

import { FIT_PADDING, fitViewport, placeFloatingBox } from "./fit.ts";

const box = (x1: number, y1: number, x2: number, y2: number) => ({
  x1,
  y1,
  x2,
  y2,
  w: x2 - x1,
  h: y2 - y1,
});
const ZOOM = { min: 0.05, max: 10 };

// Nothing is covered, so there is nothing to correct for: `null` sends the
// caller to cytoscape's own fit, which keeps that case byte-identical.
test("no gutter falls through to cytoscape's own fit", () => {
  assert.equal(fitViewport(box(0, 0, 100, 100), 800, 600, 0, ZOOM), null);
});

test("a gutter wider than the canvas falls through too", () => {
  assert.equal(fitViewport(box(0, 0, 100, 100), 200, 600, 190, ZOOM), null);
});

test("a degenerate bounding box falls through", () => {
  assert.equal(fitViewport(box(5, 5, 5, 5), 800, 600, 260, ZOOM), null);
});

test("the zoom fits the tighter axis of the uncovered strip", () => {
  // 800 wide with 260 covered leaves 540; padding takes 80 off each axis.
  const v = fitViewport(box(0, 0, 100, 50), 800, 600, 260, ZOOM)!;
  assert.equal(v.zoom, Math.min((540 - 2 * FIT_PADDING) / 100, (600 - 2 * FIT_PADDING) / 50));
  assert.equal(v.zoom, 4.6);
});

test("the pan centres the box in the strip, not in the whole canvas", () => {
  const v = fitViewport(box(0, 0, 100, 50), 800, 600, 260, ZOOM)!;
  // Centre of the 540px strip, i.e. 270 — well left of the canvas centre (400).
  assert.equal(v.pan.x + v.zoom * 50, 270);
  assert.equal(v.pan.y + v.zoom * 25, 300);
});

// minZoom wins over maxZoom on a clash, exactly as cytoscape resolves it.
test("the zoom is clamped into the instance's range", () => {
  const huge = fitViewport(box(0, 0, 1, 1), 800, 600, 260, ZOOM)!;
  assert.equal(huge.zoom, ZOOM.max);
  const tiny = fitViewport(box(0, 0, 1e6, 1e6), 800, 600, 260, ZOOM)!;
  assert.equal(tiny.zoom, ZOOM.min);
  assert.equal(fitViewport(box(0, 0, 1, 1), 800, 600, 260, { min: 12, max: 10 })!.zoom, 12);
});

const CANVAS = { width: 800, height: 600, gutter: 260 };

test("a floating box sits below-right of its anchor when there is room", () => {
  assert.deepEqual(placeFloatingBox({ x: 100, y: 100 }, { width: 200, height: 80 }, CANVAS), {
    left: 114,
    top: 114,
  });
});

// The panel column is what the box has to stay clear of, not the window edge.
test("it flips left rather than sliding under the panel column", () => {
  const placed = placeFloatingBox({ x: 480, y: 100 }, { width: 200, height: 80 }, CANVAS);
  assert.equal(placed.left, 480 - 14 - 200);
});

test("it flips up at the bottom edge", () => {
  const placed = placeFloatingBox({ x: 100, y: 560 }, { width: 200, height: 80 }, CANVAS);
  assert.equal(placed.top, 560 - 14 - 80);
});

test("it never leaves the canvas, even when it fits nowhere", () => {
  const placed = placeFloatingBox({ x: 5, y: 5 }, { width: 2000, height: 2000 }, CANVAS);
  assert.deepEqual(placed, { left: 8, top: 8 });
});
