import { apiFetch } from "./client";
import type { Clause, SearchResult } from "../types/api";

// Diverged from core's `service::search_state::SavedSearch`: core stores
// clauses/results/sort_prefs as untyped JSON (`Vec<Value>`/`Map`) and has no
// `updated_at` (added by the route) — not generatable until core types them.
export interface SearchState {
  clauses: Clause[];
  source: string;
  max_results: number;
  results: SearchResult[];
  saved_ids: string[];
  sort_prefs: Record<string, string> | null;
  updated_at: string;
}

export async function getSearchHistory(prefix: string, limit = 10): Promise<string[]> {
  const params = new URLSearchParams({ prefix, limit: String(limit) });
  const data = await apiFetch<{ suggestions: string[] }>(`/api/search/history?${params}`);
  return data.suggestions;
}

export async function getSearchState(): Promise<SearchState | null> {
  const data = await apiFetch<{ state: SearchState | null }>("/api/search/state");
  return data.state;
}

export async function appendSavedId(sourceId: string): Promise<void> {
  const state = await getSearchState();
  if (!state) return;
  if (state.saved_ids.includes(sourceId)) return;
  await saveSearchState(
    state.clauses,
    state.source,
    state.max_results,
    state.results,
    [...state.saved_ids, sourceId],
    state.sort_prefs,
  );
}

export async function saveSearchState(
  clauses: Clause[],
  source: string,
  maxResults: number,
  results: SearchResult[],
  savedIds: string[],
  sortPrefs: Record<string, string> | null = null,
): Promise<void> {
  await apiFetch("/api/search/state", {
    method: "POST",
    body: JSON.stringify({
      clauses,
      source,
      max_results: maxResults,
      results,
      saved_ids: savedIds,
      sort_prefs: sortPrefs,
    }),
  });
}
