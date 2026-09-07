import { libraryFetch } from "../stores/backend.ts";
import type {
  ArxivFetchBody,
  ArxivFetchResponse,
  ArxivSearchBody,
  ArxivSearchResponse,
  DoiResolveBody,
  DoiResolveResponse,
  DoiSaveBody,
  DoiSaveResponse,
  OpenAlexSaveBody,
  OpenAlexSaveResponse,
  OpenAlexSearchBody,
  OpenAlexSearchResponse,
} from "../types/api";

export type { ArxivFetchResponse, ArxivSearchResponse, OpenAlexSearchResponse };

export type ArxivSort = "relevance" | "newest" | "oldest" | "lastUpdated";

export async function searchArxiv(
  query: string,
  maxResults = 25,
  save = false,
  sort: ArxivSort = "relevance",
): Promise<ArxivSearchResponse> {
  const body: ArxivSearchBody = { query, max_results: maxResults, save, sort };
  return libraryFetch<ArxivSearchResponse>("/api/arxiv/search", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

export async function fetchArxiv(
  sourceId: string,
  save = true
): Promise<ArxivFetchResponse> {
  const body: ArxivFetchBody = { source_id: sourceId, save };
  return libraryFetch<ArxivFetchResponse>("/api/arxiv/fetch", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

export async function resolveDoi(doi: string): Promise<DoiResolveResponse> {
  const body: DoiResolveBody = { doi };
  return libraryFetch("/api/doi/resolve", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

export async function saveDoi(doi: string): Promise<DoiSaveResponse> {
  const body: DoiSaveBody = { doi };
  return libraryFetch("/api/doi/save", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

export type OpenAlexSort = "relevance" | "newest" | "oldest" | "citations";

export async function searchOpenAlex(
  query: string,
  maxResults = 25,
  sort: OpenAlexSort = "relevance",
): Promise<OpenAlexSearchResponse> {
  const body: OpenAlexSearchBody = { query, max_results: maxResults, sort };
  return libraryFetch<OpenAlexSearchResponse>("/api/openalex/search", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

export async function saveOpenAlex(
  sourceId: string,
): Promise<OpenAlexSaveResponse> {
  const body: OpenAlexSaveBody = { source_id: sourceId };
  return libraryFetch("/api/openalex/save", {
    method: "POST",
    body: JSON.stringify(body),
  });
}
