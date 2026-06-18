import { useState, useCallback, useEffect, useMemo } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import { Spinner } from "../components/ui/spinner";
import { QueryBuilder, makeClause } from "../components/search/QueryBuilder";
import { ResultRow } from "../components/search/ResultRow";
import {
  searchArxiv,
  fetchArxiv,
  searchOpenAlex,
  saveOpenAlex,
  type ArxivSort,
  type OpenAlexSort,
} from "../api/search";
import { getSearchState, saveSearchState } from "../api/searchState";
import { getSettings } from "../api/settings";
import { listPapers } from "../api/papers";
import type { Clause, SearchResult, Paper } from "../types/api";
import { isArxivId, normalizeAuthors } from "../lib/papers";

// ── helpers ──────────────────────────────────────────────────────────────────

function paperMatchesText(paper: Paper, query: string): boolean {
  const q = query.toLowerCase().trim();
  if (!q) return false;
  const title = paper.title.toLowerCase();
  const summary = (paper.summary ?? "").toLowerCase();
  const authors = normalizeAuthors(paper.authors).join(" ").toLowerCase();
  return title.includes(q) || summary.includes(q) || authors.includes(q);
}

// Adapts the storage Paper model to the SearchResult wire shape.
// paper.url (storage field) maps to paper_url (API contract field renamed from pdf_url per ADR 0011).
function paperToSearchResult(paper: Paper): SearchResult {
  return {
    source_id: paper.source_id,
    version: paper.version,
    title: paper.title,
    summary: paper.summary ?? "",
    authors: normalizeAuthors(paper.authors),
    published: paper.published ?? "",
    paper_url: paper.url ?? "",
    primary_category: paper.category ?? "",
    // Reconstruct namespaced entry_id (e.g. "arxiv:2204.12985") to match the API path contract.
    // Fallback to bare source_id when source is null (BibTeX/PDF imports); those papers are always
    // pre-saved (isSaved=true) so entry_id is never used for dispatch in that case.
    entry_id: paper.source ? `${paper.source}:${paper.source_id}` : paper.source_id,
  };
}

function mergeResults(existing: SearchResult[] | null, incoming: SearchResult[]): SearchResult[] {
  const seen = new Set((existing ?? []).map((r) => r.source_id));
  return [...(existing ?? []), ...incoming.filter((r) => !seen.has(r.source_id))];
}

// ── view sort (client-side, all sources) ─────────────────────────────────────

type ViewSort = "default" | "newest" | "oldest";

function applyViewSort(list: SearchResult[], sort: ViewSort): SearchResult[] {
  if (sort === "default") return list;
  // Undated entries use "0000-00-00" so they always sink to the bottom of both orderings.
  return [...list].sort((a, b) => {
    const da = a.published || "0000-00-00";
    const db = b.published || "0000-00-00";
    if (da === db) return 0;
    // ISO date strings sort correctly via plain < / > (year-month-day order).
    return sort === "newest"
      ? da < db ? 1 : -1
      : da < db ? -1 : 1;
  });
}

// ── sort prefs ────────────────────────────────────────────────────────────────

const ARXIV_SORT_VALUES = ["relevance", "newest", "oldest", "lastUpdated"] as const;
const OPENALEX_SORT_VALUES = ["relevance", "newest", "oldest", "citations"] as const;

interface SortPrefs extends Record<string, string> {
  arxivSort: ArxivSort;
  openAlexSort: OpenAlexSort;
}

const SORT_DEFAULTS: SortPrefs = {
  arxivSort: "relevance",
  openAlexSort: "relevance",
};

function parseSortPrefs(raw: Record<string, string> | null | undefined): SortPrefs {
  if (!raw) return SORT_DEFAULTS;
  return {
    arxivSort: (ARXIV_SORT_VALUES as readonly string[]).includes(raw["arxivSort"] ?? "")
      ? (raw["arxivSort"] as ArxivSort)
      : SORT_DEFAULTS.arxivSort,
    openAlexSort: (OPENALEX_SORT_VALUES as readonly string[]).includes(raw["openAlexSort"] ?? "")
      ? (raw["openAlexSort"] as OpenAlexSort)
      : SORT_DEFAULTS.openAlexSort,
  };
}

// ── constants ────────────────────────────────────────────────────────────────

const LOCAL_SEARCH_PAPER_LIMIT = 500;
const SOURCES = ["arxiv", "openalex", "local"] as const;
type Source = (typeof SOURCES)[number];
const MAX_RESULT_OPTIONS = [10, 25, 50, 100] as const;

// ── component ────────────────────────────────────────────────────────────────

export default function SearchPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [queryText, setQueryText] = useState("");
  const [source, setSource] = useState<Source>("arxiv");
  const [maxResults, setMaxResults] = useState<number>(25);
  const [sortPrefs, setSortPrefs] = useState<SortPrefs>(SORT_DEFAULTS);
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [advClauses, setAdvClauses] = useState<Clause[]>([makeClause()]);

  const [results, setResults] = useState<SearchResult[] | null>(null);
  const [savedIds, setSavedIds] = useState<Set<string>>(new Set());
  const [isAppending, setIsAppending] = useState(false);
  const [viewSort, setViewSort] = useState<ViewSort>("default");

  const displayedResults = useMemo(
    () => (results ? applyViewSort(results, viewSort) : null),
    [results, viewSort],
  );

const { data: settings } = useQuery({ queryKey: ["settings"], queryFn: getSettings });
  const historyEnabled = settings?.search_history_enabled !== false;

  // Restore search state from db on first mount.
  const [restored, setRestored] = useState(false);
  useEffect(() => {
    getSearchState()
      .then((state) => {
        if (state) {
          // Restore query text from first persisted clause
          const firstClause = state.clauses?.[0];
          if (firstClause?.value) setQueryText(firstClause.value);
          if (SOURCES.includes(state.source as Source)) setSource(state.source as Source);
          setMaxResults(state.max_results);
          setResults(state.results);
          setSavedIds(new Set(state.saved_ids));
          setSortPrefs(parseSortPrefs(state.sort_prefs));
        }
      })
      .catch((err) => console.warn("Failed to restore search state:", err))
      .finally(() => setRestored(true));
  }, []);

  // arXiv search mutation
  const arxivSearch = useMutation({
    mutationFn: ({ query, sort }: { query: string; sort: ArxivSort }) =>
      searchArxiv(query, maxResults, false, sort),
  });

  // OpenAlex search mutation
  const openAlexSearch = useMutation({
    mutationFn: ({ query, sort }: { query: string; sort: OpenAlexSort }) =>
      searchOpenAlex(query, maxResults, sort),
  });

  // Local search mutation
  const localSearch = useMutation({
    mutationFn: async (query: string) => {
      const { papers } = await listPapers(LOCAL_SEARCH_PAPER_LIMIT);
      return papers
        .filter((p) => paperMatchesText(p, query))
        .map(paperToSearchResult);
    },
  });

  const isLoading = arxivSearch.isPending || openAlexSearch.isPending || localSearch.isPending;
  const isReplacing = isLoading && !isAppending;
  const error =
    (arxivSearch.error as Error | null)?.message ??
    (openAlexSearch.error as Error | null)?.message ??
    (localSearch.error as Error | null)?.message ??
    null;

  function persistState(
    query: string,
    src: Source,
    max: number,
    res: SearchResult[],
    saved: string[],
    prefs: SortPrefs,
  ) {
    const clause = [{ operator: "AND" as const, field: "all" as const, value: query, uid: "persisted" }];
    saveSearchState(clause, src, max, res, saved, prefs).catch(() => {});
  }

  // Core search runner — accepts explicit prefs so sort-change re-search works without waiting for state.
  function runSearch(mode: "replace" | "append", overridePrefs?: SortPrefs) {
    const query = queryText.trim();
    if (!query) return;
    const prefs = overridePrefs ?? sortPrefs;

    if (mode === "replace") {
      setResults(null);
      setIsAppending(false);
      setViewSort("default");
    } else {
      setIsAppending(true);
    }

    const base = results ?? [];
    const baseSaved = savedIds;

    if (source === "arxiv") {
      arxivSearch.mutate({ query, sort: prefs.arxivSort }, {
        onSuccess: (data) => {
          const merged = mode === "append" ? mergeResults(base, data.results) : data.results;
          const mergedSaved = mode === "append"
            ? new Set([...baseSaved, ...data.saved_source_ids])
            : new Set(data.saved_source_ids);
          setResults(merged);
          setSavedIds(mergedSaved);
          persistState(query, source, maxResults, merged, [...mergedSaved], prefs);
        },
        onSettled: () => { if (mode === "append") setIsAppending(false); },
      });
    } else if (source === "openalex") {
      openAlexSearch.mutate({ query, sort: prefs.openAlexSort }, {
        onSuccess: (data) => {
          const merged = mode === "append" ? mergeResults(base, data.results) : data.results;
          const newIds = new Set(data.results.map((r) => r.source_id));
          const mergedSaved = mode === "append"
            ? new Set(baseSaved)
            : new Set([...baseSaved].filter((id) => newIds.has(id)));
          setResults(merged);
          setSavedIds(mergedSaved);
          persistState(query, source, maxResults, merged, [...mergedSaved], prefs);
        },
        onSettled: () => { if (mode === "append") setIsAppending(false); },
      });
    } else {
      localSearch.mutate(query, {
        onSuccess: (data) => {
          const merged = mode === "append" ? mergeResults(base, data) : data;
          const mergedSaved = mode === "append"
            ? new Set([...baseSaved, ...data.map((r) => r.source_id)])
            : new Set(data.map((r) => r.source_id));
          setResults(merged);
          setSavedIds(mergedSaved);
          persistState(query, source, maxResults, merged, [...mergedSaved], prefs);
        },
        onSettled: () => { if (mode === "append") setIsAppending(false); },
      });
    }
  }

  const handleSearch = () => runSearch("replace");
  const handleAppend = () => runSearch("append");

  function handleClear() {
    setResults(null);
    setSavedIds(new Set());
    setIsAppending(false);
    persistState(queryText, source, maxResults, [], [], sortPrefs);
  }

  function handleSortChange(field: "arxivSort" | "openAlexSort", value: string) {
    setSortPrefs((p) => ({ ...p, [field]: value } as SortPrefs));
  }

  const handleSavePaper = useCallback(async (sourceId: string) => {
    if (isArxivId(sourceId)) {
      await fetchArxiv(sourceId, true);
    } else if (/^W\d+$/.test(sourceId)) {
      await saveOpenAlex(sourceId);
    } else {
      // Local-search results (doi:, local: source types) are always pre-saved so isSaved=true
      // in handleCheck prevents reaching here. Throw to surface any unexpected call site.
      throw new Error(`Unknown source ID format: ${sourceId}`);
    }
    setSavedIds((prev) => new Set([...prev, sourceId]));
    // Refresh library/stats consumers and trip the graph's dirty flag.
    queryClient.invalidateQueries({ queryKey: ["papers"] });
    queryClient.invalidateQueries({ queryKey: ["stats"] });
  }, [queryClient]);

  const handleViewPdf = useCallback((result: SearchResult, isSaved: boolean) => {
    navigate("/pdf-preview", { state: { result, isSaved } });
  }, [navigate]);

  if (!restored) {
    return (
      <div className="flex flex-col h-full items-center justify-center text-[var(--color-muted)]">
        <Spinner size={28} />
      </div>
    );
  }

  const hasQuery = queryText.trim().length > 0;

  return (
    <div className="flex flex-col h-full">
      {/* ── Fixed header + controls ─────────────────────────────────────────── */}
      <div
        className="shrink-0 px-6 pt-6 pb-4 border-b border-[var(--color-border)]"
        style={{ background: "var(--color-bg)" }}
      >
        <h1 className="font-display text-[27px] font-semibold leading-tight tracking-[-0.015em] text-text mb-4">
          Search Papers
        </h1>

        {/* Row 1: source + search input + advanced toggle */}
        <div className="flex items-center gap-2 mb-2">
          {/* Source segmented control */}
          <div className="flex rounded-md border border-border overflow-hidden text-sm shrink-0">
            {SOURCES.map((s) => (
              <button
                key={s}
                type="button"
                onClick={() => {
                  setSource(s);
                  if (s !== "arxiv") setAdvancedOpen(false);
                }}
                disabled={isLoading}
                className={[
                  "px-3 py-1.5 transition-colors disabled:opacity-50 disabled:cursor-not-allowed",
                  source === s
                    ? "bg-accent text-white font-medium"
                    : "bg-panel text-muted hover:text-text",
                ].join(" ")}
              >
                {s === "arxiv" ? "arXiv" : s === "openalex" ? "OpenAlex" : "Local"}
              </button>
            ))}
          </div>

          {/* Main search text input */}
          <input
            type="text"
            className="flex-1 h-[34px] rounded-md px-3 text-sm bg-[var(--color-bg)] text-[var(--color-text)] border border-[var(--color-border)] focus:outline-none focus:border-[var(--color-accent)] disabled:opacity-50"
            placeholder={
              source === "arxiv"
                ? "Search arXiv…"
                : source === "openalex"
                ? "Search OpenAlex…"
                : "Search local library…"
            }
            value={queryText}
            onChange={(e) => setQueryText(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter") handleSearch(); }}
            disabled={isLoading}
          />

          {/* Advanced toggle — arXiv only */}
          {source === "arxiv" && (
            <button
              type="button"
              onClick={() => setAdvancedOpen((v) => !v)}
              className="shrink-0 h-[34px] px-3 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-panel)] text-[var(--color-text)] hover:opacity-80 transition-opacity"
            >
              Advanced {advancedOpen ? "▴" : "▾"}
            </button>
          )}
        </div>

        {/* Advanced panel — arXiv only */}
        {source === "arxiv" && advancedOpen && (
          <div
            className="mb-3 p-3 rounded-md border border-[var(--color-border)]"
            style={{ background: "var(--color-panel)" }}
          >
            <QueryBuilder
              clauses={advClauses}
              onChange={setAdvClauses}
              onInsert={(q) => { setQueryText(q); setAdvancedOpen(false); }}
              historyEnabled={historyEnabled}
            />
          </div>
        )}

        {/* Row 2: options + search buttons */}
        <div className="flex items-center gap-3 flex-wrap">
          {/* Max results */}
          {source !== "local" && (
            <div className="flex items-center gap-1.5 text-sm text-[var(--color-muted)]">
              <span>Max</span>
              <select
                className="h-[30px] rounded-md px-2 text-sm bg-[var(--color-bg)] text-[var(--color-text)] border border-[var(--color-border)] focus:outline-none focus:border-[var(--color-accent)] disabled:opacity-50 disabled:cursor-not-allowed"
                value={maxResults}
                onChange={(e) => setMaxResults(Number(e.target.value))}
                disabled={isLoading}
                aria-label="Maximum results"
              >
                {MAX_RESULT_OPTIONS.map((n) => (
                  <option key={n} value={n}>{n}</option>
                ))}
              </select>
            </div>
          )}

          {/* Sort — per source, auto-re-searches on change */}
          {source === "arxiv" && (
            <div className="flex items-center gap-1.5 text-sm text-[var(--color-muted)]">
              <span>Sort</span>
              <select
                className="h-[30px] rounded-md px-2 text-sm bg-[var(--color-bg)] text-[var(--color-text)] border border-[var(--color-border)] focus:outline-none focus:border-[var(--color-accent)] disabled:opacity-50 disabled:cursor-not-allowed"
                value={sortPrefs.arxivSort}
                onChange={(e) => handleSortChange("arxivSort", e.target.value)}
                disabled={isLoading}
                aria-label="Sort by"
              >
                <option value="relevance">Relevance</option>
                <option value="newest">Newest</option>
                <option value="oldest">Oldest</option>
                <option value="lastUpdated">Last Updated</option>
              </select>
            </div>
          )}

          {source === "openalex" && (
            <div className="flex items-center gap-1.5 text-sm text-[var(--color-muted)]">
              <span>Sort</span>
              <select
                className="h-[30px] rounded-md px-2 text-sm bg-[var(--color-bg)] text-[var(--color-text)] border border-[var(--color-border)] focus:outline-none focus:border-[var(--color-accent)] disabled:opacity-50 disabled:cursor-not-allowed"
                value={sortPrefs.openAlexSort}
                onChange={(e) => handleSortChange("openAlexSort", e.target.value)}
                disabled={isLoading}
                aria-label="Sort by"
              >
                <option value="relevance">Relevance</option>
                <option value="newest">Newest</option>
                <option value="oldest">Oldest</option>
                <option value="citations">Most Cited</option>
              </select>
            </div>
          )}

          <div className="flex-1" />

          {/* +/Search/− button group */}
          <div className="flex rounded-md overflow-hidden shrink-0 border border-[var(--color-border)]">
            <button
              type="button"
              onClick={handleAppend}
              disabled={isLoading || results === null || !hasQuery}
              title="Append to current results"
              aria-label="Append to current results"
              className="flex items-center justify-center w-7 py-1 text-xs font-bold transition-opacity hover:opacity-80 active:opacity-70 disabled:opacity-30 disabled:pointer-events-none"
              style={{ background: "var(--color-success)", color: "var(--color-bg)" }}
            >
              {isAppending ? <Spinner size={11} /> : "+"}
            </button>
            <button
              type="button"
              onClick={handleSearch}
              disabled={isLoading || !hasQuery}
              className="flex items-center justify-center gap-1 px-4 py-1 text-xs font-semibold tracking-wide transition-opacity hover:opacity-80 active:opacity-70 disabled:opacity-30 disabled:pointer-events-none border-x border-black/20"
              style={{ background: "var(--color-accent)", color: "var(--color-bg)" }}
            >
              {isReplacing && <Spinner size={11} />}
              {isReplacing ? "Searching…" : "Search"}
            </button>
            <button
              type="button"
              onClick={handleClear}
              disabled={isLoading || results === null}
              title="Clear results"
              aria-label="Clear results"
              className="flex items-center justify-center w-7 py-1 text-xs font-bold transition-opacity hover:opacity-80 active:opacity-70 disabled:opacity-30 disabled:pointer-events-none"
              style={{ background: "var(--color-danger)", color: "white" }}
            >
              −
            </button>
          </div>
        </div>
      </div>

      {/* ── Scrollable results area ──────────────────────────────────────────── */}
      <div className="flex-1 overflow-y-auto">
        {error && (
          <p className="px-6 py-4 text-sm" style={{ color: "var(--color-danger)" }}>
            {error}
          </p>
        )}

        {isReplacing && (
          <div className="flex flex-col items-center justify-center gap-3 py-16 text-[var(--color-muted)]">
            <Spinner size={28} />
            <span className="text-sm">Searching…</span>
          </div>
        )}

        {!isReplacing && results !== null && results.length === 0 && (
          <p className="px-6 py-8 text-sm text-[var(--color-muted)] text-center">
            No results found.
          </p>
        )}

        {!isReplacing && displayedResults && displayedResults.length > 0 && (
          <div>
            {/* Results header: count + view sort */}
            <div className="flex items-center gap-3 px-6 py-2 border-b border-[var(--color-border)]">
              <span className="text-xs text-[var(--color-muted)]">
                {results!.length} result{results!.length !== 1 ? "s" : ""}
                {isAppending && <span className="ml-2 text-[var(--color-accent)]">appending…</span>}
              </span>
              <div className="flex-1" />
              <div className="flex items-center gap-1.5 text-xs text-[var(--color-muted)]">
                <span>Sort</span>
                <select
                  className="h-[26px] rounded px-1.5 text-xs bg-[var(--color-bg)] text-[var(--color-text)] border border-[var(--color-border)] focus:outline-none focus:border-[var(--color-accent)]"
                  value={viewSort}
                  onChange={(e) => setViewSort(e.target.value as ViewSort)}
                  aria-label="Sort results"
                >
                  <option value="default">Default</option>
                  <option value="newest">Newest first</option>
                  <option value="oldest">Oldest first</option>
                </select>
              </div>
            </div>
            {displayedResults.map((result) => (
              <ResultRow
                key={result.source_id}
                result={result}
                saved={savedIds.has(result.source_id)}
                onSave={handleSavePaper}
                onViewPdf={handleViewPdf}
              />
            ))}
            {isAppending && (
              <div className="flex items-center gap-2 px-6 py-3 text-xs border-t border-[var(--color-border)]" style={{ color: "var(--color-muted)" }}>
                <Spinner size={12} />
                <span>Fetching more…</span>
              </div>
            )}
          </div>
        )}

        {!isReplacing && results === null && !error && (
          <p className="px-6 py-8 text-sm text-[var(--color-muted)] text-center">
            Enter search terms above and press Search.
          </p>
        )}
      </div>
    </div>
  );
}
