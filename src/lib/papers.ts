import type { Paper } from "../types/api";

// Source ids may already carry a "arxiv:"/"openalex:"/"doi:" prefix, so strip it
// before re-prefixing to avoid "arXiv:arxiv:..." double labels.
export function labelForSource(paper: Paper): string | null {
  const id = paper.source_id;
  if (paper.source === "arxiv" || id.startsWith("arxiv:"))
    return `arXiv:${id.replace(/^arxiv:/, "")}`;
  if (paper.source === "openalex" || id.startsWith("openalex:"))
    return `OpenAlex:${id.replace(/^openalex:/, "")}`;
  if (id.startsWith("doi:")) return `DOI:${id.replace(/^doi:/, "")}`;
  if (id.startsWith("local:")) return "Local";
  return id || null;
}

// Covers modern arXiv IDs (YYMM.NNNNN) and legacy IDs (cs/0612047, math.GT/0309136, cond-mat.mes-hall/0309136).
// Extends formats/markdown.py _ARXIV_ID_RE to handle dotted-category prefixes.
const _ARXIV_ID_RE = /^\d{4}\.\d{4,5}(v\d+)?$|^[a-z][a-z-]+(\.[a-z][a-z-]*)?\/\d{7}(v\d+)?$/;

export function isArxivId(sourceId: string): boolean {
  return _ARXIV_ID_RE.test(sourceId);
}
