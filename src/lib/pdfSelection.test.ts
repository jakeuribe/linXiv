// Run: node --experimental-transform-types --test src/lib/pdfSelection.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { normalizePdfSelection, pdfSelectionText } from "./pdfSelection.ts";

// Minimal structural fakes (no jsdom): an element node whose `closest`
// answers the .react-pdf__Page containment question directly.
function el(inPage: boolean) {
  return {
    nodeType: 1,
    parentElement: null,
    closest: (sel: string) => (sel === ".react-pdf__Page" && inPage ? {} : null),
  } as unknown as Node;
}
function selection(
  start: Node,
  end: Node,
  text: string,
  collapsed = false
): Selection {
  return {
    isCollapsed: collapsed,
    rangeCount: 1,
    getRangeAt: () => ({ startContainer: start, endContainer: end }),
    toString: () => text,
  } as unknown as Selection;
}

test("normalizePdfSelection collapses PDF text-layer whitespace", () => {
  assert.equal(
    normalizePdfSelection("Attention  Is\nAll \n You  Need"),
    "Attention Is All You Need",
  );
  assert.equal(normalizePdfSelection("  \n  "), "");
  assert.equal(normalizePdfSelection("soft\u00ADhyphen"), "softhyphen");
  // A soft hyphen at a PDF line break rejoins the word \u2014 no phantom space.
  assert.equal(normalizePdfSelection("trans\u00AD\nformer model"), "transformer model");
});

test("pdfSelectionText requires both ends inside a PDF page", () => {
  const inside = el(true);
  const outside = el(false);
  assert.equal(pdfSelectionText(selection(inside, inside, "in  pdf")), "in pdf");
  // Drag started in the PDF but released over the editor: rejected.
  assert.equal(pdfSelectionText(selection(inside, outside, "mixed")), "");
  assert.equal(pdfSelectionText(selection(outside, inside, "mixed")), "");
  assert.equal(pdfSelectionText(selection(outside, outside, "ui text")), "");
  assert.equal(pdfSelectionText(selection(inside, inside, "x", true)), "");
  assert.equal(pdfSelectionText(null), "");
});
