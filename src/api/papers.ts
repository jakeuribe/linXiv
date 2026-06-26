import { apiFetch, BASE_URL, isTauri } from "./client";
import type { Paper, PaperVersionsResponse } from "../types/api";

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

/**
 * Returns the URL to stream/download the PDF for a paper. In Tauri this hits
 * the backend directly; in browser dev it goes through the Vite proxy.
 */
export function getPaperPdfUrl(sourceId: string, version?: number): string {
  const query = version !== undefined ? `?version=${version}` : "";
  const id = encodeURIComponent(sourceId);
  if (isTauri) return linxivUrl(`papers/${id}/pdf${query}`);
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
