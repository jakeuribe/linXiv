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

  const rects: AnchorRect[] = [];
  const clamp = (n: number) => Math.max(0, Math.min(1, n));
  for (const r of Array.from(range.getClientRects())) {
    if (r.width === 0 || r.height === 0) continue;
    // A non-empty rect outside the page box means the selection spans a page
    // break; the quote would cover both pages but only the start page renders.
    if (r.bottom <= box.top || r.top >= box.bottom) return null;
    const x = clamp((r.left - box.left) / box.width);
    const y = clamp((r.top - box.top) / box.height);
    const w = Math.min(clamp(r.width / box.width), 1 - x);
    const h = Math.min(clamp(r.height / box.height), 1 - y);
    if (w === 0 || h === 0) continue;
    rects.push({ x, y, w, h });
  }
  if (rects.length === 0 || rects.length > MAX_RECTS) return null;
  return { v: 1, version, page: pageNum, color, quote, rects };
}
