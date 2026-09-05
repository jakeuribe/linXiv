// Text pulled out of a react-pdf text layer: hard line breaks between spans,
// doubled spaces, and soft hyphens all survive into the raw selection string.
// A soft hyphen swallows the line break after it (that's what it hyphenated),
// so "trans\u00AD\nformer" rejoins as "transformer", not "trans former".
export function normalizePdfSelection(text: string): string {
  return text.replace(/\u00AD\s*/g, "").replace(/\s+/g, " ").trim();
}

// Node.ELEMENT_NODE as a literal so this module runs under plain node --test
// (no DOM globals).
const ELEMENT_NODE = 1;

function inPdfPage(node: Node): boolean {
  const el = node.nodeType === ELEMENT_NODE ? (node as Element) : node.parentElement;
  return Boolean(el?.closest(".react-pdf__Page"));
}

/** Scoping core, DOM-free for tests: text of `sel` only when BOTH ends sit
 * inside a rendered PDF page (same `.react-pdf__Page` scoping PdfReader uses
 * for highlights) \u2014 a drag that starts in the PDF and ends over the editor
 * would otherwise return mixed PDF+UI text. Returns "" for
 * no/collapsed/out-of-scope selections. */
export function pdfSelectionText(sel: Selection | null): string {
  if (!sel || sel.isCollapsed || sel.rangeCount === 0) return "";
  const range = sel.getRangeAt(0);
  if (!inPdfPage(range.startContainer) || !inPdfPage(range.endContainer)) return "";
  return normalizePdfSelection(sel.toString());
}

/** Current browser selection, scoped per `pdfSelectionText`. */
export function getPdfSelectionText(): string {
  return pdfSelectionText(window.getSelection());
}
