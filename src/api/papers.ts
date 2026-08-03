import { apiFetch, BASE_URL, isTauri } from "./client";
import type { Paper, PaperVersionsResponse, DoiVersionCandidate } from "../types/api";

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

export async function listPapers(
  limit = 200,
  offset = 0
): Promise<{ papers: Paper[] }> {
  return apiFetch<{ papers: Paper[] }>(
    `/api/papers?limit=${limit}&offset=${offset}`
  );
}

export async function getPaper(sourceId: string): Promise<Paper> {
  return apiFetch<Paper>(`/api/papers/${encodeURIComponent(sourceId)}`);
}

export async function getPaperBySfk(sfk: number, version?: number): Promise<Paper> {
  const query = version !== undefined ? `?version=${version}` : "";
  return apiFetch<Paper>(`/api/papers/sfk/${sfk}${query}`);
}

export async function getPaperVersions(sfk: number): Promise<PaperVersionsResponse> {
  return apiFetch<PaperVersionsResponse>(`/api/papers/sfk/${sfk}/versions`);
}

export async function getDoiVersionCandidates(sfk: number): Promise<DoiVersionCandidate[]> {
  const data = await apiFetch<{ candidates: DoiVersionCandidate[] }>(
    `/api/papers/sfk/${sfk}/doi-candidates`
  );
  return data.candidates;
}

export async function deletePaper(sourceId: string): Promise<{ deleted: string }> {
  return apiFetch<{ deleted: string }>(
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

export async function removeFromAllProjects(sfk: number): Promise<{ ok: boolean; removed_from: number[] }> {
  return apiFetch(`/api/papers/sfk/${sfk}/projects`, { method: "DELETE" });
}

export async function repairPaper(sfk: number, body: PaperRepairBody): Promise<Paper> {
  return apiFetch<Paper>(`/api/papers/sfk/${sfk}`, {
    method: "PUT",
    body: JSON.stringify(body),
  });
}

export async function searchLibrary(
  q: string,
  limit = 50
): Promise<{ papers: Paper[] }> {
  return apiFetch<{ papers: Paper[] }>(
    `/api/papers/search?q=${encodeURIComponent(q)}&limit=${limit}`
  );
}

export interface FullTextResult {
  source_id: string;
  version: number;
  indexed: boolean;
  chars?: number;
  reason?: string;
}

/**
 * Downloads a paper's arXiv TeX source and indexes it, so `searchLibrary` can
 * match on the body and not just the metadata. arXiv-only; already-indexed
 * papers are skipped unless `force`.
 */
export async function fetchFullText(
  sourceId: string,
  force = false
): Promise<FullTextResult> {
  return apiFetch<FullTextResult>(
    `/api/papers/${encodeURIComponent(sourceId)}/full-text${force ? "?force=true" : ""}`,
    { method: "POST" }
  );
}

/**
 * How many stored papers still have no indexed TeX source — the backlog the
 * background full-text worker is working through. Counts papers with nothing to
 * fetch (non-arXiv) too, so it can plateau above zero.
 */
export async function fullTextPending(): Promise<{ pending: number }> {
  return apiFetch<{ pending: number }>("/api/papers/full-text-pending");
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
