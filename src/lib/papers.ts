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

/** arXiv detection for library papers, whose source_id keeps the "arxiv:"
 *  namespace (unlike search results' stripped ids — those use isArxivId).
 *  Prefix only: PROVIDER defaults to "arxiv" on legacy rows, so paper.source
 *  can't answer this. */
export function isArxivPaper(paper: Paper): boolean {
  return paper.source_id.startsWith("arxiv:");
}

/** The one scheme gate for URLs handed to the OS opener: url fields are free
 *  text (imports, metadata edits, upstream APIs), so only http(s) qualifies. */
export function isHttpUrl(url: string): boolean {
  return /^https?:\/\//i.test(url);
}

/** The paper's external landing URL, matching the detail page's links: the
 *  DOI resolver first, else the source's own URL (http(s) only). */
export function landingUrl(paper: Paper): string | null {
  if (paper.doi) return `https://doi.org/${paper.doi}`;
  return paper.url && isHttpUrl(paper.url) ? paper.url : null;
}
