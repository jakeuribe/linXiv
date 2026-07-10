// PDF highlight anchors. An annotation's `anchor` column is the JSON of an
// `Anchor`. Rects are normalized 0..1 to the page box so they survive the
// reader's fit-to-width resizing (the rendered page width changes with the
// container).

export interface AnchorRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Anchor {
  v: 1;
  version: number; // paper version the coords were measured against
  page: number; // 1-based page number
  color: string;
  quote: string;
  rects: AnchorRect[];
}

// Zotero-ish palette. First entry is the default.
export const HIGHLIGHT_COLORS = [
  "#ffd400",
  "#7ac34a",
  "#54b9f5",
  "#ff8ac4",
  "#ff6b5e",
] as const;

const MAX_RECTS = 500;
const MAX_QUOTE_LEN = 10_000;

function isRect(r: unknown): r is AnchorRect {
  if (typeof r !== "object" || r === null) return false;
  const v = r as Record<string, unknown>;
  const inUnit = (n: unknown): n is number =>
    typeof n === "number" && n >= 0 && n <= 1;
  // Coords/sizes are normalized to the page box; reject out-of-range or
  // zero-area rects, and rects that extend past the page box edges.
  return (
    inUnit(v.x) &&
    inUnit(v.y) &&
    inUnit(v.w) &&
    inUnit(v.h) &&
    (v.w as number) > 0 &&
    (v.h as number) > 0 &&
    (v.x as number) + (v.w as number) <= 1 + 1e-9 &&
    (v.y as number) + (v.h as number) <= 1 + 1e-9
  );
}

// `anchor` is a trust boundary (settable via the annotations API/MCP/CLI):
// validate shape, page/version, color, and every rect before rendering; bad
// anchor → null.
export function parseAnchor(raw: string | null | undefined): Anchor | null {
  if (!raw) return null;
  try {
    const a = JSON.parse(raw) as Anchor;
    if (
      a &&
      a.v === 1 &&
      Number.isInteger(a.page) &&
      a.page >= 1 &&
      Number.isInteger(a.version) &&
      a.version >= 0 &&
      typeof a.color === "string" &&
      /^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$/.test(
        a.color,
      ) &&
      typeof a.quote === "string" &&
      a.quote.length <= MAX_QUOTE_LEN &&
      Array.isArray(a.rects) &&
      a.rects.length > 0 &&
      a.rects.length <= MAX_RECTS &&
      a.rects.every(isRect)
    ) {
      return a;
    }
  } catch {
    // malformed anchor → treat the note as a plain note, not a highlight
  }
  return null;
}

// A raw (un-normalized) client rect in viewport coordinates.
export interface RawRect {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

// pdf.js renders each word/run of the text layer as its own <span>, so a
// selection's getClientRects() comes back one tight rect PER WORD with nothing
// for the inter-word spaces — painting them directly gives a patchy highlight
// with white gaps. Group rects that share a visual line (a rect joins a line
// when its vertical midpoint falls inside that line's band, tolerant of the
// sub-pixel top/height jitter between adjacent spans) and merge each group into
// one span from min-left to max-right. Pure so it's unit-testable without a DOM.
export function coalesceRectsIntoLines(rects: RawRect[]): RawRect[] {
  const sorted = [...rects].sort((a, b) => a.top - b.top || a.left - b.left);
  const lines: RawRect[] = [];
  for (const r of sorted) {
    const line = lines[lines.length - 1];
    const mid = (r.top + r.bottom) / 2;
    if (line && mid >= line.top && mid <= line.bottom) {
      line.left = Math.min(line.left, r.left);
      line.right = Math.max(line.right, r.right);
      line.top = Math.min(line.top, r.top);
      line.bottom = Math.max(line.bottom, r.bottom);
    } else {
      lines.push({ ...r });
    }
  }
  return lines;
}

// Build an anchor from the current text selection, measured against the
// `.react-pdf__Page` element the selection starts in. Returns null when there is
// no usable selection (collapsed, empty, or not inside a rendered page).
export function selectionToAnchor(version: number, color: string): Anchor | null {
  const sel = window.getSelection();
  if (!sel || sel.isCollapsed || sel.rangeCount === 0) return null;
  const quote = sel.toString().trim().slice(0, MAX_QUOTE_LEN);
  if (!quote) return null;

  const range = sel.getRangeAt(0);
  const startNode = range.startContainer;
  const startEl =
    startNode.nodeType === Node.ELEMENT_NODE
      ? (startNode as Element)
      : startNode.parentElement;
  const pageEl = startEl?.closest<HTMLElement>(".react-pdf__Page");
  if (!pageEl) return null;

  const pageNum = Number(pageEl.getAttribute("data-page-number"));
  const box = pageEl.getBoundingClientRect();
  if (!pageNum || box.width === 0 || box.height === 0) return null;

  const raw: RawRect[] = [];
  for (const r of Array.from(range.getClientRects())) {
    if (r.width === 0 || r.height === 0) continue;
    // A non-empty rect outside the page box means the selection spans a page
    // break; the quote would cover both pages but only the start page renders.
    if (r.bottom <= box.top || r.top >= box.bottom) return null;
    raw.push({ left: r.left, top: r.top, right: r.right, bottom: r.bottom });
  }

  const rects: AnchorRect[] = [];
  const clamp = (n: number) => Math.max(0, Math.min(1, n));
  for (const line of coalesceRectsIntoLines(raw)) {
    const x = clamp((line.left - box.left) / box.width);
    const y = clamp((line.top - box.top) / box.height);
    const w = Math.min(clamp((line.right - line.left) / box.width), 1 - x);
    const h = Math.min(clamp((line.bottom - line.top) / box.height), 1 - y);
    if (w === 0 || h === 0) continue;
    rects.push({ x, y, w, h });
  }
  if (rects.length === 0 || rects.length > MAX_RECTS) return null;
  return { v: 1, version, page: pageNum, color, quote, rects };
}
