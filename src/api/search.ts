import { libraryFetch } from "../stores/backend.ts";
import type {
  ArxivFetchResponse,
  ArxivSearchResponse,
  DoiResolveResponse,
  DoiSaveResponse,
  OpenAlexSaveResponse,
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
  return libraryFetch<ArxivSearchResponse>("/api/arxiv/search", {
    method: "POST",
    body: JSON.stringify({ query, max_results: maxResults, save, sort }),
  });
}

export async function fetchArxiv(
  sourceId: string,
  save = true
): Promise<ArxivFetchResponse> {
  return libraryFetch<ArxivFetchResponse>("/api/arxiv/fetch", {
    method: "POST",
    body: JSON.stringify({ source_id: sourceId, save }),
  });
}

export async function resolveDoi(doi: string): Promise<DoiResolveResponse> {
  return libraryFetch("/api/doi/resolve", {
    method: "POST",
    body: JSON.stringify({ doi }),
  });
}

export async function saveDoi(doi: string): Promise<DoiSaveResponse> {
  return libraryFetch("/api/doi/save", {
    method: "POST",
    body: JSON.stringify({ doi }),
  });
}

export type OpenAlexSort = "relevance" | "newest" | "oldest" | "citations";

export async function searchOpenAlex(
  query: string,
  maxResults = 25,
  sort: OpenAlexSort = "relevance",
): Promise<OpenAlexSearchResponse> {
  return libraryFetch<OpenAlexSearchResponse>("/api/openalex/search", {
    method: "POST",
    body: JSON.stringify({ query, max_results: maxResults, sort }),
  });
}

export async function saveOpenAlex(
  sourceId: string,
): Promise<OpenAlexSaveResponse> {
  return libraryFetch("/api/openalex/save", {
    method: "POST",
    body: JSON.stringify({ source_id: sourceId }),
  });
}
