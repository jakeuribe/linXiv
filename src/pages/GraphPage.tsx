import { Suspense, lazy, useCallback, useDeferredValue, useEffect, useMemo, useRef, useState } from "react";
import { useLocation, useNavigate } from "react-router";
import { keepPreviousData, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { AlertCircle, Network } from "lucide-react";

import { useThemeStore } from "../stores/theme";
import { useUiStore } from "../stores/ui";
import { getColors } from "../lib/theme";
import { getGraphView } from "../api/graph";
import { listProjects } from "../api/projects";
import {
  addToProjectMutationOptions,
  createProjectMutationOptions,
  onGraphDirtying,
} from "../lib/paperMutations";
import { indexView } from "../lib/graph/model";
import type { GraphFilterState } from "../lib/graph/filter";
import { EMPTY_FILTER, joinTypes, matchGraph, noMatchCause } from "../lib/graph/filter";
import type { ForceSettings } from "../lib/graph/layout";
import { DEFAULT_FORCES } from "../lib/graph/layout";
import type { GraphCanvasHandle } from "../components/graph/GraphCanvas";
import GraphPanels from "../components/graph/GraphPanels";
import { Spinner } from "../components/ui/spinner";
import { Button } from "../components/ui/button";
import { formSubmitOnCtrlEnter } from "../lib/submitShortcut";
import { Dialog } from "../components/ui/dialog";
import { Input } from "../components/ui/input";
import { EmptyState } from "../components/ui/empty-state";

// Root query keys whose invalidation may change graph-relevant data.
//
// This is the SECOND of the two ways this page hears about stale data, and the
// weaker one: react-query emits an `invalidate` cache event only for queries
// that are actually in the cache, so a key listed here is heard only while some
// page happens to hold a query under it. It stays because it is the only thing
// that covers the call sites which invalidate directly instead of going through
// the registry in src/lib/paperMutations.ts (StorageSection's blanket
// invalidate, the ORCID backfill). The registry's own `onGraphDirtying` signal
// below is what makes the operations it owns reliable.
// cytoscape and d3-force are ~400kB of the bundle and are needed by exactly one
// screen. AppShell imports this page eagerly (it is keep-alive, so it must exist
// from boot), so a lazy PAGE would not help — the canvas is the boundary that
// does: it is not rendered until the first visit to /graph, which is the same
// point the old iframe used to be mounted at. Every user used to pay for those
// two libraries only on opening the graph, and this is what keeps that true.
const GraphCanvas = lazy(() => import("../components/graph/GraphCanvas"));

const GRAPH_DIRTYING_KEYS = new Set([
  "stats", "papers", "paper", "projects", "project", "tags", "tag", "authors", "author",
]);

/**
 * The Knowledge Graph.
 *
 * This page used to be a thin host around an `<iframe>` running a 2,400-line
 * unbundled browser script, and most of what it did was work around that frame:
 * a postMessage protocol in both directions, a `graph_loaded` reply carrying the
 * load state because the guest owned the canvas and the host owned the spinner,
 * an eight-second fallback for a reply that never came, a theme push on every
 * palette change, a `?api=` parameter naming which backend the guest should talk
 * to, and a hand-back channel for keyboard shortcuts — key events do not cross a
 * frame boundary, so every app-wide shortcut was dead on /graph alone.
 *
 * None of that survives the port. The canvas is a component, the load state is
 * react-query's, the theme is read from the store, and the shortcuts are the
 * window's own.
 */
export default function GraphPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const canvasRef = useRef<GraphCanvasHandle>(null);
  const preset = useThemeStore((s) => s.preset);
  const mode = useThemeStore((s) => s.mode);
  const overrides = useThemeStore((s) => s.overrides);
  const overrideAlphas = useThemeStore((s) => s.overrideAlphas);
  const hideSingleAuthors = useUiStore((s) => s.hideSingleAuthors);
  const setHideSingleAuthors = useUiStore((s) => s.setHideSingleAuthors);

  const theme = useMemo(
    () => getColors(preset, mode, overrides, overrideAlphas),
    [preset, mode, overrides, overrideAlphas]
  );

  const [filter, setFilter] = useState<GraphFilterState>(EMPTY_FILTER);
  const [forces, setForces] = useState<ForceSettings>(DEFAULT_FORCES);
  const [selectedIds, setSelectedIds] = useState<ReadonlySet<string>>(() => new Set());
  const [projectPickerOpen, setProjectPickerOpen] = useState(false);
  const [projectPickerError, setProjectPickerError] = useState<string | null>(null);
  const [newProjectName, setNewProjectName] = useState("");
  const [dirty, setDirty] = useState(false);

  // AppShell's keep-alive renders this page from app BOOT and hides it with
  // `display: none`. A hidden container lays out 0x0, and cytoscape's fit bails
  // silently on a zero-sized viewport — so a graph built there would keep the
  // default zoom 1 with its layout spread off-screen. Every user would also pay
  // the graph fetch plus the force layout at startup without ever opening this
  // page. Mount on the first visit instead; it then stays mounted, so leaving
  // and coming back still keeps the settled layout.
  const onGraphRoute = useLocation().pathname === "/graph";
  const [visited, setVisited] = useState(false);
  useEffect(() => {
    if (onGraphRoute) setVisited(true);
  }, [onGraphRoute]);

  const {
    data: view,
    isPending,
    isFetching,
    error,
    refetch,
    dataUpdatedAt,
  } = useQuery({
    queryKey: ["graph", hideSingleAuthors],
    queryFn: () => getGraphView(hideSingleAuthors),
    enabled: visited,
    // "Hide single-paper authors" is applied by the BACKEND, so toggling it is a
    // different query key — one with nothing cached under it. Without this,
    // `data` would drop to undefined for the length of that fetch, unmounting
    // the canvas and the panels: the settled positions and the last viewport
    // live in refs inside GraphCanvas and die with it, so the payload that came
    // back would be seeded as a COLD load and reframed, and the panels would
    // re-collapse. A checkbox next to Refresh would silently throw away an
    // arrangement the user built. Holding the previous payload keeps both
    // mounted, so the new one arrives as the in-place reload it is meant to be:
    // surviving nodes keep their positions and the viewport is held.
    placeholderData: keepPreviousData,
    // This query fetches when it is ASKED to and at no other time. A new payload
    // rebuilds the simulation, which re-anneals the layout from alpha 1 — so any
    // fetch the user did not ask for drifts an arrangement they may have spent a
    // while making, and can yank a grabbed node out from under a drag. The three
    // settings below are the three ways react-query would otherwise start one on
    // its own; the invalidation side is held by `refetchType: "none"` in
    // src/lib/paperMutations.ts. Refresh calls `refetch()`, which ignores all of
    // this and is the point.
    staleTime: Infinity,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
  });

  const index = useMemo(() => (view ? indexView(view) : null), [view]);

  // Typing in a filter box re-matches every paper. Deferring it lets React keep
  // the keystroke responsive and drop superseded passes on its own — the fixed
  // 280ms debounce this replaces was a guess that was always either laggy or
  // wasteful, depending on the library.
  const deferredFilter = useDeferredValue(filter);
  const match = useMemo(
    () => (view && index ? matchGraph(view, index, deferredFilter) : null),
    [view, index, deferredFilter]
  );

  // Selected papers the current filter state does not DRAW at all: everything
  // when the Papers checkbox is off, and the non-matching ones under isolate.
  const hiddenSelectedCount = useMemo(() => {
    if (!match || selectedIds.size === 0) return 0;
    if (match.hiddenTypes.has("paper")) return selectedIds.size;
    if (!match.isolate) return 0;
    let n = 0;
    for (const id of selectedIds) if (!match.papers.has(id)) n++;
    return n;
  }, [match, selectedIds]);

  const selectedSourceIds = useMemo(() => {
    if (!index) return [];
    const out: string[] = [];
    for (const id of selectedIds) {
      const source = index.paperById.get(id)?.source_id;
      if (source) out.push(source);
    }
    return out;
  }, [index, selectedIds]);

  const projectPickerUi = {
    setError: setProjectPickerError,
    // The shared partial-failure contract (src/lib/paperMutations.ts) re-selects
    // exactly the papers that could not be added, so a retry can't re-add the
    // ones that made it in. It speaks `source_id`, which the canvas does not —
    // map back through the payload. (Across the iframe this needed a round trip
    // and could leave the two copies of the selection disagreeing.)
    selectFailures: (sourceIds: string[]) => {
      if (!index) return;
      const wanted = new Set(sourceIds);
      const next = new Set<string>();
      for (const [id, paper] of index.paperById) {
        if (wanted.has(paper.source_id)) next.add(id);
      }
      setSelectedIds(next);
    },
    onDone: () => {
      setProjectPickerOpen(false);
      setSelectedIds(new Set());
    },
    clearName: () => setNewProjectName(""),
  };

  const addToProjectMutation = useMutation(
    addToProjectMutationOptions(queryClient, projectPickerUi)
  );
  const createProjectMutation = useMutation(
    createProjectMutationOptions(queryClient, projectPickerUi)
  );

  const { data: projectsData, isLoading: projectsLoading } = useQuery({
    queryKey: ["projects"],
    queryFn: () => listProjects(),
    enabled: projectPickerOpen,
  });

  // Flag the Refresh button when a query holding graph-relevant data is
  // invalidated elsewhere (this page is keep-alive, so it sees those events).
  // Bumped on every dirtying signal; `handleRefresh` snapshots it so a change
  // that lands WHILE a refresh is in flight is not cleared by that refresh's
  // success — the payload it fetched predates the change.
  const dirtyEpoch = useRef(0);
  const markDirty = useCallback(() => {
    dirtyEpoch.current++;
    setDirty(true);
  }, []);
  useEffect(() => {
    const unsubscribe = queryClient.getQueryCache().subscribe((event) => {
      if (event.type !== "updated" || event.action.type !== "invalidate") return;
      const root = event.query.queryKey[0];
      if (typeof root === "string" && GRAPH_DIRTYING_KEYS.has(root)) markDirty();
    });
    return unsubscribe;
  }, [queryClient, markDirty]);

  // The primary signal: the invalidation registry announcing an operation that
  // changes what `/api/graph` would return. Unlike the cache subscription above
  // it does not depend on another page holding a matching query — an author
  // merge from /authors or a project retag from /projects reaches the graph even
  // when nothing has ["authors"] or ["projects"] cached.
  useEffect(() => onGraphDirtying(markDirty), [markDirty]);

  // The dot means "the graph on screen is older than the library", so what
  // clears it is DATA ARRIVING — not which control asked for it. Hanging that
  // off the Refresh button alone got it wrong in both directions: clearing up
  // front told the user they were current when the refresh then failed, and
  // clearing only there left the dot lit after a "Hide single-paper authors"
  // toggle had already re-fetched and redrawn from a payload that included the
  // change. `dataUpdatedAt` moves only on a SUCCESSFUL fetch, so a failure
  // leaves the dot alone by construction, and the epoch guard keeps a change
  // that landed while the fetch was in flight from being cleared by it — that
  // payload predates the change.
  const fetchEpoch = useRef(0);
  useEffect(() => {
    if (isFetching) fetchEpoch.current = dirtyEpoch.current;
  }, [isFetching]);
  useEffect(() => {
    if (!dataUpdatedAt) return;
    if (dirtyEpoch.current === fetchEpoch.current) setDirty(false);
  }, [dataUpdatedAt]);

  const handleRefresh = useCallback(() => {
    void refetch();
  }, [refetch]);

  // The panel column is `position: absolute` over the canvas's right edge, so a
  // plain fit would push the rightmost nodes — and their right-hand labels,
  // which stick out further still — underneath the panels. Measure what it
  // covers and let the canvas frame into the strip that is left.
  const panelsRef = useRef<HTMLDivElement>(null);
  const [gutter, setGutter] = useState(0);
  // Read live by the canvas's fit, which cannot wait for this state to commit —
  // see GraphCanvas's `measureGutter`. The state above still drives what RENDERS
  // (the no-match notice's centring, the hover inspector's flip point), where a
  // re-render is exactly what is wanted.
  const measureGutter = useCallback(
    () => panelsRef.current?.getBoundingClientRect().width ?? 0,
    []
  );
  useEffect(() => {
    const el = panelsRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const measure = () => setGutter(el.getBoundingClientRect().width);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [view]);

  const handlePaperTap = useCallback(
    (id: string, additive: boolean) => {
      if (additive) {
        setSelectedIds((prev) => {
          const next = new Set(prev);
          if (!next.delete(id)) next.add(id);
          return next;
        });
        return;
      }
      // A click that leaves the graph drops the selection: this page stays
      // mounted across the route change, so a selection left behind comes back
      // highlighted with an action bar for papers the user has moved on from.
      setSelectedIds(new Set());
      navigate(`/library/${id}`);
    },
    [navigate]
  );

  const handleAuthorTap = useCallback(
    (authorId: number) => {
      setSelectedIds(new Set());
      navigate(`/authors/${authorId}`);
    },
    [navigate]
  );

  // The same target TagBadge links to everywhere else in the app; TagPage
  // lowercases the param itself, so the node's display casing is fine.
  const handleTagTap = useCallback(
    (label: string) => {
      setSelectedIds(new Set());
      navigate(`/tags/${encodeURIComponent(label)}`);
    },
    [navigate]
  );

  const handleSelectAllVisible = useCallback(() => {
    if (!match || match.hiddenTypes.has("paper")) return;
    setSelectedIds(new Set(match.papers));
  }, [match]);

  const clearSelection = useCallback(() => setSelectedIds(new Set()), []);
  const clearFilters = useCallback(() => setFilter(EMPTY_FILTER), []);

  const ready = view && index && match;
  const empty = ready && view.papers.length + view.authors.length + view.tags.length === 0;

  return (
    <div className="w-full h-full flex flex-col">
      <div className="p-4 border-b border-border flex items-center gap-3">
        <h1 className="font-display text-[27px] font-semibold leading-tight tracking-[-0.015em] text-text">
          Knowledge Graph
        </h1>
        <span className="text-sm text-muted">
          {selectedIds.size > 0
            ? `${selectedIds.size} paper${selectedIds.size !== 1 ? "s" : ""} selected; Ctrl/Cmd+click to add more`
            : "Click a node to open · Ctrl/Cmd+click to select"}
        </span>
        <div className="ml-auto flex items-center gap-4">
          {/* A refetch that failed with a graph still drawn underneath says so
              here instead of covering that graph with the error card — the view
              the user panned and zoomed to is still valid. */}
          {error && view && (
            <span
              role="status"
              className="text-sm max-w-[32ch] truncate"
              style={{ color: "var(--color-danger)" }}
              title={String((error as Error).message ?? error)}
            >
              Refresh failed: {String((error as Error).message ?? error)}
            </span>
          )}
          <Button
            variant="ghost"
            size="sm"
            onClick={handleRefresh}
            disabled={isFetching}
            title={
              dirty
                ? "Graph data has changed since it was loaded. Click to refresh"
                : "Reload the graph from the latest data"
            }
          >
            {isFetching ? "Refreshing…" : "Refresh"}
            {dirty && !isFetching && (
              <span
                aria-hidden
                className="inline-block w-1.5 h-1.5 rounded-full align-middle"
                style={{ backgroundColor: "var(--color-accent)" }}
              />
            )}
          </Button>
          <label
            className="flex items-center gap-2 text-sm text-muted cursor-pointer select-none"
            title="Drop authors linked to only one paper to declutter the graph. They leave the payload entirely, so the graph's own Author filter can't match them either"
          >
            <input
              type="checkbox"
              checked={hideSingleAuthors}
              onChange={(e) => setHideSingleAuthors(e.target.checked)}
            />
            Hide single-paper authors
          </label>
        </div>
      </div>

      <div className="flex-1 relative overflow-hidden" style={{ backgroundColor: "var(--color-bg)" }}>
        {ready && !empty && (
          <>
            <Suspense fallback={<Spinner size={28} />}>
              <GraphCanvas
                ref={canvasRef}
                view={view}
                index={index}
                theme={theme}
                forces={forces}
                match={match}
                selectedIds={selectedIds}
                gutter={gutter}
                measureGutter={measureGutter}
                onPaperTap={handlePaperTap}
                onAuthorTap={handleAuthorTap}
                onTagTap={handleTagTap}
                onBackgroundTap={clearSelection}
              />
            </Suspense>
            {/* The canvas is the one surface in the app with no "no results"
                state: a filter matching nothing leaves either a blank rectangle
                (under isolate) or a field of 8% ghosts, and neither is
                distinguishable from a graph that failed to load. It cannot be a
                full-bleed overlay either — that would bury the very panels the
                user needs to undo the filter — so it sits in the strip the panel
                column leaves uncovered. */}
            <NoMatchNotice
              match={match}
              gutter={gutter}
              authorFilter={deferredFilter.author.trim()}
              excludeSingleAuthors={hideSingleAuthors}
              onClearFilters={clearFilters}
              onShowSingleAuthors={() => setHideSingleAuthors(false)}
            />
            <GraphPanels
              columnRef={panelsRef}
              view={view}
              filter={filter}
              onFilterChange={setFilter}
              onClearFilters={clearFilters}
              forces={forces}
              onForcesChange={setForces}
              onRelayout={() => canvasRef.current?.relayout()}
              selectedCount={selectedIds.size}
              hiddenSelectedCount={hiddenSelectedCount}
              onSelectAllVisible={handleSelectAllVisible}
              onClearSelection={clearSelection}
            />
          </>
        )}

        {(!ready || empty) && (
          <div
            className="absolute inset-0 overflow-y-auto flex items-center justify-center"
            style={{ backgroundColor: "var(--color-bg)" }}
          >
            {isPending || !visited ? (
              <Spinner size={28} />
            ) : error ? (
              <EmptyState
                icon={<AlertCircle size={28} strokeWidth={1.5} />}
                title="Couldn't load the graph"
                description={`The graph data could not be fetched: ${
                  (error as Error).message ?? String(error)
                }`}
                actionLabel={isFetching ? "Retrying…" : "Retry"}
                onAction={handleRefresh}
              />
            ) : (
              <EmptyState
                icon={<Network size={28} strokeWidth={1.5} />}
                title="Nothing to graph yet"
                description="The knowledge graph is drawn from your library — import a few papers and they'll appear here, linked by their authors and tags."
                actionLabel="Go to Library"
                onAction={() => navigate("/library")}
              />
            )}
          </div>
        )}
      </div>

      {selectedIds.size > 0 && (
        <div
          className="shrink-0 flex items-center justify-between px-6 py-3 border-t border-border shadow-lg"
          style={{ backgroundColor: "var(--color-panel)" }}
        >
          <span className="text-sm font-medium text-text">{selectedIds.size} selected</span>
          <div className="flex items-center gap-2">
            <Button variant="ghost" size="sm" onClick={clearSelection}>
              Clear
            </Button>
            <Button variant="muted" size="sm" onClick={() => setProjectPickerOpen(true)}>
              Add to Project
            </Button>
          </div>
        </div>
      )}

      <Dialog
        open={projectPickerOpen}
        onClose={() => {
          setProjectPickerOpen(false);
          setProjectPickerError(null);
          setNewProjectName("");
          createProjectMutation.reset();
        }}
        title="Add to Project"
      >
        <div className="space-y-3">
          {projectPickerError && (
            <p className="text-sm" style={{ color: "var(--color-danger)" }}>
              {projectPickerError}
            </p>
          )}
          {projectsLoading ? (
            <div className="flex items-center justify-center py-4">
              <Spinner size={20} />
            </div>
          ) : !projectsData?.projects?.length ? (
            <div className="space-y-2">
              <p className="text-muted text-sm">No projects yet.</p>
              <form
                onSubmit={(e) => {
                  e.preventDefault();
                  const name = newProjectName.trim();
                  if (name) createProjectMutation.mutate({ name, sourceIds: selectedSourceIds });
                }}
                onKeyDown={formSubmitOnCtrlEnter}
                className="flex gap-2"
              >
                <Input
                  autoFocus
                  value={newProjectName}
                  onChange={(e) => setNewProjectName(e.target.value)}
                  placeholder="New project name…"
                  className="flex-1 text-sm"
                  disabled={createProjectMutation.isPending}
                />
                <Button
                  type="submit"
                  variant="primary"
                  size="sm"
                  disabled={!newProjectName.trim() || createProjectMutation.isPending}
                >
                  {createProjectMutation.isPending ? "Creating…" : "Create"}
                </Button>
              </form>
            </div>
          ) : (
            <div className="space-y-2 max-h-64 overflow-y-auto">
              {projectsData.projects.map((project) => (
                <button
                  type="button"
                  key={project.id}
                  onClick={() =>
                    addToProjectMutation.mutate({
                      projectId: project.id,
                      sourceIds: selectedSourceIds,
                    })
                  }
                  disabled={addToProjectMutation.isPending}
                  className="w-full text-left px-3 py-2 rounded-md border border-border hover:border-[var(--color-accent)] hover:text-[var(--color-accent)] text-text text-sm transition-colors disabled:opacity-50"
                >
                  {project.name}
                  {project.description && (
                    <span className="block text-xs text-muted mt-0.5 truncate">
                      {project.description}
                    </span>
                  )}
                </button>
              ))}
            </div>
          )}
          <div className="flex justify-end pt-2">
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                setProjectPickerOpen(false);
                setProjectPickerError(null);
                setNewProjectName("");
                createProjectMutation.reset();
              }}
            >
              Cancel
            </Button>
          </div>
        </div>
      </Dialog>
    </div>
  );
}

/**
 * The Filters > Author box matches `GraphPaper.author_keys`, and "Hide
 * single-paper authors" is applied by the BACKEND — it drops those authors from
 * that index too. So with the option on, typing a name that is certainly in the
 * library empties the canvas under "No papers match the active filters": true,
 * but not why. The checkbox lives in the page header, out of the canvas's way,
 * so this is the only place that can say so.
 */
const AUTHOR_HIDDEN_HINT =
  "Authors with a single paper are hidden, so the Author filter cannot match them.";

function NoMatchNotice({
  match,
  gutter,
  authorFilter,
  excludeSingleAuthors,
  onClearFilters,
  onShowSingleAuthors,
}: {
  match: ReturnType<typeof matchGraph>;
  gutter: number;
  authorFilter: string;
  excludeSingleAuthors: boolean;
  onClearFilters: () => void;
  onShowSingleAuthors: () => void;
}) {
  if (match.drawnCount > 0) return null;
  // Three Visibility checkboxes off is a different mistake from a filter that
  // excludes everything, and "Clear all filters" fixes both, so one notice with
  // two bodies covers it.
  const cause = noMatchCause(match);
  const hiddenByVisibility = cause.kind === "visibility";
  // Nothing here can tell whether a hidden author is the actual cause — the
  // names never arrived — so it is offered as a second possibility, and only
  // when both halves of it are in force.
  const authorsMayBeHidden = !hiddenByVisibility && !!authorFilter && excludeSingleAuthors;
  return (
    <div
      role="status"
      aria-live="polite"
      className="absolute top-1/2 z-10 w-[min(340px,60%)] rounded-md border border-border p-4 text-center shadow-lg"
      style={{
        left: `calc((100% - ${gutter}px) / 2)`,
        transform: "translate(-50%, -50%)",
        backgroundColor: "var(--color-panel)",
      }}
    >
      <div className="text-sm font-semibold text-text">
        {hiddenByVisibility ? "Nothing to draw" : "No matches"}
      </div>
      <p className="mt-1 text-xs text-muted">
        {cause.kind === "visibility"
          ? `${joinTypes(cause.types)} ${
              cause.types.length === 1 ? "is" : "are"
            } switched off under Filters › Visibility.`
          : "No papers match the active filters."}
      </p>
      {authorsMayBeHidden && (
        <>
          <p className="mt-2 text-xs text-muted">{AUTHOR_HIDDEN_HINT}</p>
          <Button variant="muted" size="sm" className="mt-2 w-full" onClick={onShowSingleAuthors}>
            Show single-paper authors
          </Button>
        </>
      )}
      <Button variant="ghost" size="sm" className="mt-1 w-full" onClick={onClearFilters}>
        Clear all filters
      </Button>
    </div>
  );
}
