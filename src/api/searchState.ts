import { libraryFetch } from "../stores/backend.ts";
import type { Clause, SearchResult } from "../types/api";

// Diverged from core's `service::search_state::SavedSearch`: core stores
// clauses/results/sort_prefs as untyped JSON (`Vec<Value>`/`Map`) and has no
// `updated_at` (added by the route) — not generatable until core types them.
// `saved_ids` still exists on the wire (the backend defaults it to []) but the
// GUI no longer reads or writes it: saved state is the ["papers","saved",...]
// react-query lookup, not a persisted snapshot.
export interface SearchState {
  clauses: Clause[];
  source: string;
  max_results: number;
  results: SearchResult[];
  sort_prefs: Record<string, string> | null;
  updated_at: string;
}

export async function getSearchHistory(prefix: string, limit = 10): Promise<string[]> {
  const params = new URLSearchParams({ prefix, limit: String(limit) });
  const data = await libraryFetch<{ suggestions: string[] }>(`/api/search/history?${params}`);
  return data.suggestions;
}

export async function getSearchState(): Promise<SearchState | null> {
  const data = await libraryFetch<{ state: SearchState | null }>("/api/search/state");
  return data.state;
}

export async function saveSearchState(
  clauses: Clause[],
  source: string,
  maxResults: number,
  results: SearchResult[],
  sortPrefs: Record<string, string> | null = null,
): Promise<void> {
  await libraryFetch("/api/search/state", {
    method: "POST",
    body: JSON.stringify({
      clauses,
      source,
      max_results: maxResults,
      results,
      sort_prefs: sortPrefs,
    }),
  });
}
