import { BASE_URL, isTauri } from "./client.ts";
import { libraryFetch } from "../stores/backend.ts";
import type {
  Paper,
  PaperVersionsResponse,
  DoiVersionCandidate,
  FullTextReceipt,
  MergeReceipt,
} from "../types/api";

// The in-process app serves PDF bytes over the `linxiv://` custom scheme (the
// invoke()-based transport can't stream binary). The webview host form differs by
// platform (Tauri docs): linxiv://localhost on Linux/macOS, http://linxiv.localhost
// on Windows. In browser dev there is no custom scheme — keep the HTTP URL so the
// Vite proxy reaches the sidecar.
function linxivUrl(path: string): string {
  const isWindows =
    typeof navigator !== "undefined" && /Windows/i.test(navigator.userAgent);
  const base = isWindows ? "http://linxiv.localhost" : "linxiv://localhost";
  return `${base}/${path}`;
}

// `{metric}_{dir}`; the server sorts (and indexes) by metric, so the ordering
// holds across the whole library, not just the fetched window.
export type PaperSort =
  | "published_desc"
  | "published_asc"
  | "added_desc"
  | "added_asc"
  | "title_asc"
  | "title_desc";

export async function listPapers(
  limit = 200,
  offset = 0,
  sort?: PaperSort,
  project?: number
): Promise<{ papers: Paper[] }> {
  const order = sort ? `&sort=${sort.split("_")[0]}&dir=${sort.split("_")[1]}` : "";
  const proj = project !== undefined ? `&project=${project}` : "";
  return libraryFetch<{ papers: Paper[] }>(
    `/api/papers?limit=${limit}&offset=${offset}${order}${proj}`
  );
}

// Server-side cap on GET /api/papers?limit=; project-scoped fetches use it so
// membership, not a 200-paper window, decides what a page shows.
// ponytail: a single project >5000 papers is still windowed; page with offset=
// if that ever becomes real.
export const PAPER_LIMIT_MAX = 5000;

/** All papers linked to a project, filtered server-side (no client window). */
export function listProjectPapers(projectId: number): Promise<{ papers: Paper[] }> {
  return listPapers(PAPER_LIMIT_MAX, 0, undefined, projectId);
}

export async function getPaper(sourceId: string): Promise<Paper> {
  return libraryFetch<Paper>(`/api/papers/${encodeURIComponent(sourceId)}`);
}

export async function getPaperBySfk(sfk: number, version?: number): Promise<Paper> {
  const query = version !== undefined ? `?version=${version}` : "";
  return libraryFetch<Paper>(`/api/papers/sfk/${sfk}${query}`);
}

export async function getPaperVersions(sfk: number): Promise<PaperVersionsResponse> {
  return libraryFetch<PaperVersionsResponse>(`/api/papers/sfk/${sfk}/versions`);
}

export async function getDoiVersionCandidates(sfk: number): Promise<DoiVersionCandidate[]> {
  const data = await libraryFetch<{ candidates: DoiVersionCandidate[] }>(
    `/api/papers/sfk/${sfk}/doi-candidates`
  );
  return data.candidates;
}

// Merge a duplicate paper root INTO the paper `winnerSfk` (the open paper's
// metadata stays canonical; the duplicate's notes, annotations, memberships,
// tags, missing versions and PDFs move over, then the duplicate is deleted).
// 409s on self/trashed/share-linked duplicates.
export async function mergePapers(
  winnerSfk: number,
  loserSfk: number
): Promise<MergeReceipt> {
  return libraryFetch<MergeReceipt>(`/api/papers/sfk/${winnerSfk}/merge`, {
    method: "POST",
    body: JSON.stringify({ loser_source_fk: loserSfk }),
  });
}

export async function deletePaper(sourceId: string): Promise<{ deleted: string }> {
  return libraryFetch<{ deleted: string }>(
    `/api/papers/${encodeURIComponent(sourceId)}`,
    { method: "DELETE" }
  );
}

export interface PaperRepairBody {
  title: string;
  authors: string[];
  published: string;
  summary: string;
  category?: string | null;
  doi?: string | null;
  url?: string | null;
  tags?: string[] | null;
}

export async function removeFromAllProjects(sfk: number): Promise<{ ok: boolean; removed_from_projects: number[] }> {
  return libraryFetch(`/api/papers/sfk/${sfk}/projects`, { method: "DELETE" });
}

export async function repairPaper(sfk: number, body: PaperRepairBody): Promise<Paper> {
  return libraryFetch<Paper>(`/api/papers/sfk/${sfk}`, {
    method: "PUT",
    body: JSON.stringify(body),
  });
}

export async function searchLibrary(
  q: string,
  limit = 50
): Promise<{ papers: Paper[] }> {
  return libraryFetch<{ papers: Paper[] }>(
    `/api/papers/search?q=${encodeURIComponent(q)}&limit=${limit}`
  );
}

/** Core's `FullTextReceipt`. */
export type { FullTextReceipt as FullTextResult } from "../types/api";

/**
 * Downloads a paper's arXiv TeX source and indexes it, so `searchLibrary` can
 * match on the body and not just the metadata. arXiv-only; already-indexed
 * papers are skipped unless `force`.
 */
export async function fetchFullText(
  sourceId: string,
  force = false
): Promise<FullTextReceipt> {
  return libraryFetch<FullTextReceipt>(
    `/api/papers/${encodeURIComponent(sourceId)}/full-text${force ? "?force=true" : ""}`,
    { method: "POST" }
  );
}

/**
 * How many stored arXiv papers still have no indexed TeX source — the backlog
 * the background full-text worker is working through.
 */
export async function fullTextPending(): Promise<{ pending: number }> {
  return libraryFetch<{ pending: number }>("/api/papers/full-text-pending");
}

/**
 * Returns the URL to stream/download the PDF for a paper. In Tauri this hits
 * the backend directly; in browser dev it goes through the Vite proxy.
 */
export function getPaperPdfUrl(sourceId: string, version?: number): string {
  const id = encodeURIComponent(sourceId);
  // Tauri: id travels as a query param (a slash-bearing old-style id stays one
  // token). Browser dev: the HTTP path the Vite proxy forwards to the sidecar.
  if (isTauri) {
    const v = version !== undefined ? `&version=${version}` : "";
    return linxivUrl(`pdf?id=${id}${v}`);
  }
  const query = version !== undefined ? `?version=${version}` : "";
  return `${BASE_URL}/api/papers/${id}/pdf${query}`;
}

/**
 * URL that streams an external (arXiv) PDF through the host-allowlisted proxy.
 * Used by the preview pages' CORS fallback. linxiv:// in the app, HTTP in dev.
 */
export function getPdfProxyUrl(remoteUrl: string): string {
  const url = encodeURIComponent(remoteUrl);
  if (isTauri) return linxivUrl(`pdf-proxy?url=${url}`);
  return `${BASE_URL}/api/pdf/proxy?url=${url}`;
}
