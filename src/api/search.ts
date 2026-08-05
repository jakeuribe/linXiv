import { apiFetch } from "./client";
import type { SearchResult } from "../types/api";

export interface ArxivSearchResponse {
  results: SearchResult[];
  saved_source_ids: string[];
}

export type ArxivSort = "relevance" | "newest" | "oldest" | "lastUpdated";

export async function searchArxiv(
  query: string,
  maxResults = 25,
  save = false,
  sort: ArxivSort = "relevance",
): Promise<ArxivSearchResponse> {
  return apiFetch<ArxivSearchResponse>("/api/arxiv/search", {
    method: "POST",
    body: JSON.stringify({ query, max_results: maxResults, save, sort }),
  });
}

export interface ArxivFetchResponse {
  paper: SearchResult;
  saved: boolean;
  source_id: string;
}

export async function fetchArxiv(
  sourceId: string,
  save = true
): Promise<ArxivFetchResponse> {
  return apiFetch<ArxivFetchResponse>("/api/arxiv/fetch", {
    method: "POST",
    body: JSON.stringify({ source_id: sourceId, save }),
  });
}

export interface DoiMetadata {
  [key: string]: unknown;
}

export async function resolveDoi(
  doi: string
): Promise<{ metadata: DoiMetadata }> {
  return apiFetch("/api/doi/resolve", {
    method: "POST",
    body: JSON.stringify({ doi }),
  });
}

export async function saveDoi(
  doi: string
): Promise<{ metadata: DoiMetadata; saved: boolean }> {
  return apiFetch("/api/doi/save", {
    method: "POST",
    body: JSON.stringify({ doi }),
  });
}

export interface OpenAlexSearchResponse {
  results: SearchResult[];
  saved_source_ids: string[];
}

export type OpenAlexSort = "relevance" | "newest" | "oldest" | "citations";

export async function searchOpenAlex(
  query: string,
  maxResults = 25,
  sort: OpenAlexSort = "relevance",
): Promise<OpenAlexSearchResponse> {
  return apiFetch<OpenAlexSearchResponse>("/api/openalex/search", {
    method: "POST",
    body: JSON.stringify({ query, max_results: maxResults, sort }),
  });
}

export async function saveOpenAlex(
  sourceId: string,
): Promise<{ saved: boolean; source_id: string }> {
  return apiFetch("/api/openalex/save", {
    method: "POST",
    body: JSON.stringify({ source_id: sourceId }),
  });
}
