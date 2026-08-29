// What the hover inspector says about a node.
//
// Paper labels are drawn with `text-max-width: 180px` + `text-wrap: ellipsis`,
// so any real title is cut off on the canvas, while the payload already carries
// the category, published date, tags, PDF flag and the whole abstract for every
// paper. This is the "peek without navigating" affordance the rest of the app
// gives a paper row.

import type { GraphIndex, GraphNodeType } from "./model.ts";

export const SUMMARY_MAX = 260;

export interface TooltipContent {
  /** May contain TeX — arXiv titles do. The component renders it via MathText. */
  title: string;
  /** Meta lines, rendered one per line. Plain text: this is library metadata. */
  meta: string[];
  /** Truncated abstract. May contain TeX; rendered via MathText, kept separate
   *  from `meta` so only it and the title go through the TeX path. */
  summary?: string;
}

export function truncate(text: string, max: number): string {
  const t = text.replace(/\s+/g, " ").trim();
  if (t.length <= max) return t;
  const cut = t.slice(0, max);
  const space = cut.lastIndexOf(" ");
  return `${(space > max * 0.6 ? cut.slice(0, space) : cut).trimEnd()}…`;
}

function pluralPapers(n: number): string {
  return `${n} ${n === 1 ? "paper" : "papers"}`;
}

/**
 * "Author · 37 papers", plus what the filter left of those 37 when it is not all
 * of them.
 *
 * The degree is a fact about the library — the Authors page reports the same
 * number — so filtering the canvas must not silently rewrite it; but a node
 * hovered on a filtered graph stands for a set the canvas is mostly not showing.
 * Report both. With no filter in force the two are equal and the line is the
 * plain one.
 */
function degreeLine(kind: string, total: number, drawn: number): string {
  const head = `${kind} · ${pluralPapers(total)}`;
  if (drawn === total) return head;
  return head + (drawn === 0 ? " (none shown)" : ` (${drawn} shown)`);
}

/**
 * `drawnPapers` is the set of paper node ids currently painted at a non-zero
 * opacity. It is deliberately NOT "is the hovered node itself visible" — that
 * the user can see; this is how much of what the node stands for is.
 */
export function tooltipFor(
  nodeId: string,
  type: GraphNodeType,
  index: GraphIndex,
  drawnPapers: ReadonlySet<string>
): TooltipContent {
  if (type === "paper") {
    const p = index.paperById.get(nodeId);
    if (!p) return { title: "(untitled)", meta: [] };
    const meta: string[] = [];
    const head: string[] = [];
    if (p.category) head.push(p.category);
    // `published` is already null for the "no date" sentinel, so "no date" is
    // sayable rather than a bogus year 1.
    head.push(p.published ?? "No publication date");
    head.push(p.has_pdf ? "PDF" : "No PDF");
    meta.push(head.join(" · "));
    // Already deduped and spelled as the chips beside them are — the backend
    // resolves both, so this line names exactly what the canvas drew.
    if (p.tags.length) meta.push(p.tags.join(" · "));
    return {
      title: p.label || "(untitled)",
      meta,
      summary: p.summary ? truncate(p.summary, SUMMARY_MAX) : undefined,
    };
  }

  const node = type === "author" ? index.authorById.get(nodeId) : index.tagById.get(nodeId);
  if (!node) return { title: "(untitled)", meta: [] };
  const papers = index.papersByNode.get(nodeId) ?? [];
  const drawn = papers.filter((p) => drawnPapers.has(p)).length;
  return {
    title: node.label || "(untitled)",
    meta: [degreeLine(type === "author" ? "Author" : "Tag", node.paper_count, drawn)],
  };
}
