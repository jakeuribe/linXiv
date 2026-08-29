import { useCallback, useEffect, useRef, useState } from "react";
import { useLocation, useNavigate } from "react-router";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { AlertCircle, Network } from "lucide-react";
import { useThemeStore } from "../stores/theme";
import { useUiStore } from "../stores/ui";
import { useShortcutsStore } from "../stores/shortcuts";
import { activeShortcutCombos, shortcutForCombo } from "../lib/shortcuts";
import { getColors } from "../lib/theme";
import type { ThemeColors, ThemeMode } from "../lib/theme";
import { graphIframeSrc } from "../lib/graphIframeSrc";
import type { GraphApiTransport } from "../lib/graphIframeSrc";
import { graphLoadOutcome, graphNoReplyOutcome } from "../lib/graphLoadState";
import type { GraphLoadOutcome, GraphLoadState } from "../lib/graphLoadState";
import { isTauri } from "../api/client";
import { listProjects } from "../api/projects";
import {
  addToProjectMutationOptions,
  createProjectMutationOptions,
  onGraphDirtying,
} from "../lib/paperMutations";
import { getStats } from "../api/settings";
import { Spinner } from "../components/ui/spinner";
import { Button } from "../components/ui/button";
import { formSubmitOnCtrlEnter } from "../lib/submitShortcut";
import { Dialog } from "../components/ui/dialog";
import { Input } from "../components/ui/input";
import { EmptyState } from "../components/ui/empty-state";

// Per-option: 'in-place' applies via postMessage (iframe keeps its state);
// 'reload' lets the src track the live value so toggling re-bootstraps graph.js.
type ReloadStrategy = "in-place" | "reload";
const HIDE_SINGLE_AUTHORS_STRATEGY: ReloadStrategy = "in-place";

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
const GRAPH_DIRTYING_KEYS = new Set([
  "stats", "papers", "paper", "projects", "project", "tags", "tag", "authors", "author",
]);

// How long to wait for a graph_loaded reply before clearing the spinner.
const REFRESH_FALLBACK_MS = 8000;

// Which backend the guest fetches from. The same `isTauri` branch papers.ts
// takes for getPaperPdfUrl / getPdfProxyUrl, and for the same reason: inside
// the app (packaged OR `tauri dev`) the library runs in-process and is reachable
// only over the linxiv:// scheme, while in browser dev it lives behind the Vite
// `/api` proxy. The guest cannot tell the two apart — `tauri dev` serves it from
// http://localhost:5180 exactly as the browser does — so it used to sniff its
// own URL and land on the dev server's database under `tauri dev`, alone in an
// app whose every other surface was reading the in-process one.
const GRAPH_API_TRANSPORT: GraphApiTransport = isTauri ? "linxiv" : "origin";

// The iframe is a bare cytoscape canvas: a library with no papers, a backend
// that never answered and a fetch still in flight all render as the same blank
// rectangle, which is the one place in the app with no loading or empty state.
// graph.js reports `nodeCount` / `error` alongside `ok` in its `graph_loaded`
// reply and this page turns that into the app's own Spinner / EmptyState,
// overlaid on the (still full-size, so cytoscape keeps a real viewport) frame.
// Which of those a reply moves us to — including the rule that a FAILED reload
// must not take away a graph that is still drawn — lives in ../lib/graphLoadState.

export default function GraphPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const preset = useThemeStore(s => s.preset);
  const mode = useThemeStore(s => s.mode);
  const overrides = useThemeStore(s => s.overrides);
  const overrideAlphas = useThemeStore(s => s.overrideAlphas);
  const hideSingleAuthors = useUiStore(s => s.hideSingleAuthors);
  const shortcutOverrides = useShortcutsStore(s => s.overrides);
  const setHideSingleAuthors = useUiStore(s => s.setHideSingleAuthors);

  const [selectedSourceIds, setSelectedSourceIds] = useState<string[]>([]);
  const [projectPickerOpen, setProjectPickerOpen] = useState(false);
  const [projectPickerError, setProjectPickerError] = useState<string | null>(null);
  const [newProjectName, setNewProjectName] = useState("");
  const [dirty, setDirty] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [loadState, setLoadState] = useState<GraphLoadState>("loading");
  const [loadError, setLoadError] = useState<string | null>(null);
  // A reload that failed over a screen the user can still use. Reported in the
  // header rather than by covering the canvas — see ../lib/graphLoadState.
  const [refreshError, setRefreshError] = useState<string | null>(null);
  // The message listener and the timeout both need the CURRENT screen to decide
  // whether a failure may escalate, and neither can read `loadState` (the
  // listener is registered once, the timeout closes over the value it was armed
  // with). Mirrored here so both ask the same question of the same answer.
  const loadStateRef = useRef<GraphLoadState>("loading");
  const applyLoadOutcome = useCallback((outcome: GraphLoadOutcome) => {
    loadStateRef.current = outcome.state;
    setLoadState(outcome.state);
    setLoadError(outcome.error);
    setRefreshError(outcome.refreshError);
  }, []);

  // AppShell's keep-alive renders this page from app BOOT and hides it with
  // `display: none` (EditorPage carries the same warning for its own iframe).
  // An iframe inside a hidden container lays out 0x0 and graph.css sizes #cy in
  // vw/vh, so cytoscape would bootstrap against an empty viewport: its
  // getFitViewport() bails on a zero-sized container, meaning BOTH the initial
  // fit and the fit-on-settle silently do nothing and the first look at the
  // graph is an unframed slice at zoom 1. Every user would also pay the graph
  // fetch plus the force layout at startup without ever opening this page.
  // Mount the frame on the first visit instead; it then stays mounted, so
  // leaving and coming back still keeps the settled layout.
  const onGraphRoute = useLocation().pathname === "/graph";
  const [frameMounted, setFrameMounted] = useState(false);
  useEffect(() => {
    if (onGraphRoute) setFrameMounted(true);
  }, [onGraphRoute]);

  // Frozen when the frame first mounts — NOT at app boot, which can be many
  // theme/option changes earlier. The src is an iframe navigation, so a live
  // value here would reload the guest and drop its settled layout; later
  // changes ride theme_update / set_options instead.
  const boot = useRef<{ exclude: boolean; mode: ThemeMode; theme: ThemeColors } | null>(null);
  // Last option value pushed to the iframe; the option effect bails when this
  // is unchanged.
  const appliedExclude = useRef(hideSingleAuthors);
  if (frameMounted && boot.current === null) {
    boot.current = {
      exclude: hideSingleAuthors,
      mode,
      theme: getColors(preset, mode, overrides, overrideAlphas),
    };
    appliedExclude.current = hideSingleAuthors;
  }
  const refreshTimerRef = useRef<number | null>(null);
  // dirtyEpoch bumps on each dirtying invalidation; loadEpoch snapshots it at
  // each load start.
  const dirtyEpochRef = useRef(0);
  const loadEpochRef = useRef(0);

  // Observe ["stats"] (paper add/remove/repair all invalidate it) so it stays
  // active and the subscription below keeps refiring from the keep-alive page.
  useQuery({ queryKey: ["stats"], queryFn: getStats, staleTime: Infinity });

  const { data: projectsData, isLoading: projectsLoading } = useQuery({
    queryKey: ["projects"],
    queryFn: () => listProjects(),
    enabled: projectPickerOpen,
  });

  const projectPickerUi = {
    setError: setProjectPickerError,
    // The shared partial-failure contract (src/lib/paperMutations.ts) re-selects
    // exactly the papers that could not be added, so a retry can't re-add the
    // ones that made it in. On the Library page that is a local state update
    // and the whole story; here the selection is the GUEST's — this page only
    // mirrors what `selection_changed` reports — so narrowing it locally left
    // the canvas still highlighting the full set and the guest still holding
    // it, ready to post it back over this copy on the next click in the frame.
    selectFailures: (sourceIds: string[]) => {
      setSelectedSourceIds(sourceIds);
      postToIframe({ type: "set_selection", sourceIds });
    },
    onDone: () => {
      setProjectPickerOpen(false);
      setSelectedSourceIds([]);
      postToIframe({ type: "clear_selection" });
    },
    clearName: () => setNewProjectName(""),
  };

  const addToProjectMutation = useMutation(
    addToProjectMutationOptions(queryClient, projectPickerUi)
  );

  const createProjectMutation = useMutation(
    createProjectMutationOptions(queryClient, projectPickerUi)
  );

  function postToIframe(msg: object) {
    iframeRef.current?.contentWindow?.postMessage(msg, window.location.origin);
  }

  useEffect(() => {
    function onMessage(e: MessageEvent) {
      if (!e.data || typeof e.data !== "object") return;
      if (e.origin !== window.location.origin) return;
      if (e.data.type === "paper_clicked" && typeof e.data.id === "string") {
        setSelectedSourceIds([]);
        navigate(`/library/${e.data.id}`);
      } else if (e.data.type === "author_clicked" && typeof e.data.id === "string") {
        setSelectedSourceIds([]);
        navigate(`/authors/${Number(e.data.id)}`);
      } else if (e.data.type === "tag_clicked" && typeof e.data.label === "string") {
        // Same target TagBadge links to everywhere else in the app; TagPage
        // lowercases the param itself, so the node's display casing is fine.
        setSelectedSourceIds([]);
        navigate(`/tags/${encodeURIComponent(e.data.label)}`);
      } else if (e.data.type === "shortcut_key" && e.data.combo && typeof e.data.combo === "object") {
        // A keydown the guest matched against the combos we pushed it. Key
        // events don't cross a frame boundary, so without this hand-back the
        // app's own shortcuts are dead for as long as the graph has focus —
        // read the overrides live rather than closing over them, so a rebind
        // doesn't need this listener re-registered.
        shortcutForCombo(e.data.combo, useShortcutsStore.getState().overrides)?.run?.();
      } else if (e.data.type === "request_options" && typeof e.data.excludeSingleAuthors === "boolean") {
        // The guest's filtered-to-nothing notice offering to undo an option it
        // cannot reach. "Hide single-paper authors" is applied by the backend,
        // so the authors it drops are absent from the payload entirely and the
        // graph's own Author filter — which matches through the paper→author
        // edges — cannot see them. The checkbox is up here in the page header,
        // outside the frame, so the notice asks rather than acts. Flipping the
        // store is all this has to do: the option effect below sees the change
        // and posts the `set_options` reload. Read through getState() for the
        // same reason `shortcut_key` does — so the setter isn't a dependency
        // of this listener.
        useUiStore.getState().setHideSingleAuthors(e.data.excludeSingleAuthors);
      } else if (e.data.type === "selection_changed" && Array.isArray(e.data.sourceIds)) {
        setSelectedSourceIds(e.data.sourceIds);
      } else if (e.data.type === "graph_loaded") {
        if (refreshTimerRef.current !== null) {
          clearTimeout(refreshTimerRef.current);
          refreshTimerRef.current = null;
        }
        setRefreshing(false);
        // Clear dirty only on success and only if nothing changed since the load began.
        setDirty(e.data.ok === false || dirtyEpochRef.current !== loadEpochRef.current);
        // A failure that arrives over a graph already on screen leaves that
        // graph where it is and reports itself in the header; only a load with
        // nothing usable behind it escalates to the error card. nodeCount
        // counts papers + authors + tags, so 0 means the library itself is
        // empty rather than "filtered down to nothing".
        applyLoadOutcome(graphLoadOutcome(loadStateRef.current, e.data));
      }
    }
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, [navigate, applyLoadOutcome]);

  const sendTheme = useCallback(() => {
    const iframe = iframeRef.current;
    if (!iframe?.contentWindow) return;
    const colors = getColors(preset, mode, overrides, overrideAlphas);
    // `mode` rides along because light/dark is not recoverable from the eight
    // colour tokens, and the guest needs it for `color-scheme` — see
    // graphIframeSrc.
    iframe.contentWindow.postMessage({ type: "theme_update", colors, mode }, window.location.origin);
  }, [preset, mode, overrides, overrideAlphas]);

  useEffect(() => {
    sendTheme();
  }, [sendTheme]);

  // The guest cannot evaluate a `match` predicate it has no access to, so it is
  // handed the combos themselves. Resent on every rebind, and again on load
  // (the guest starts with an empty list and forwards nothing until it arrives).
  const sendShortcuts = useCallback(() => {
    const iframe = iframeRef.current;
    if (!iframe?.contentWindow) return;
    iframe.contentWindow.postMessage(
      { type: "set_shortcuts", combos: activeShortcutCombos(shortcutOverrides) },
      window.location.origin
    );
  }, [shortcutOverrides]);

  useEffect(() => {
    sendShortcuts();
  }, [sendShortcuts]);

  // Push an option change to the iframe in place; bail when the value is unchanged.
  useEffect(() => {
    if (hideSingleAuthors === appliedExclude.current) return;
    appliedExclude.current = hideSingleAuthors;
    if (HIDE_SINGLE_AUTHORS_STRATEGY === "in-place") {
      requestGraphReload({ type: "set_options", excludeSingleAuthors: hideSingleAuthors });
    }
    // 'reload': the src tracks the live value (see iframeSrc), so the iframe
    // reloads itself — nothing to post.
  }, [hideSingleAuthors]);

  // Flag the Refresh button when a query holding graph-relevant data is
  // invalidated elsewhere (GraphPage is keep-alive, so it sees those events).
  const markGraphDirty = useCallback(() => {
    dirtyEpochRef.current++;
    setDirty(true);
  }, []);

  useEffect(() => {
    const unsubscribe = queryClient.getQueryCache().subscribe((event) => {
      if (event.type !== "updated" || event.action.type !== "invalidate") return;
      const root = event.query.queryKey[0];
      if (typeof root === "string" && GRAPH_DIRTYING_KEYS.has(root)) markGraphDirty();
    });
    return unsubscribe;
  }, [queryClient, markGraphDirty]);

  // The primary signal: the invalidation registry announcing an operation that
  // changes what `/api/graph` would return. Unlike the cache subscription
  // above it does not depend on another page holding a matching query — an
  // author merge from /authors or a project retag from /projects reaches the
  // graph even when nothing has ["authors"] or ["projects"] cached.
  useEffect(() => onGraphDirtying(markGraphDirty), [markGraphDirty]);

  useEffect(() => () => {
    if (refreshTimerRef.current !== null) clearTimeout(refreshTimerRef.current);
  }, []);

  function handleClearSelection() {
    setSelectedSourceIds([]);
    postToIframe({ type: "clear_selection" });
  }

  // Arm a fallback for a graph_loaded reply that never arrives (a guest that
  // died before answering). Only a load with nothing on screen yet escalates to
  // the error state — a dropped reply to a refresh leaves the settled graph
  // alone and just re-flags the Refresh button.
  function armLoadFallback() {
    if (refreshTimerRef.current !== null) clearTimeout(refreshTimerRef.current);
    refreshTimerRef.current = window.setTimeout(() => {
      refreshTimerRef.current = null;
      setRefreshing(false);
      setDirty(true);
      applyLoadOutcome(graphNoReplyOutcome(loadStateRef.current));
    }, REFRESH_FALLBACK_MS);
  }

  // Drive an in-place reload (refresh or option toggle): snapshot the epoch, show
  // the spinner, and arm a fallback for a dropped graph_loaded reply.
  function requestGraphReload(message: object) {
    setRefreshing(true);
    // The previous attempt's message is about an attempt that is over.
    setRefreshError(null);
    loadEpochRef.current = dirtyEpochRef.current;
    postToIframe(message);
    armLoadFallback();
  }

  function handleRefresh() {
    requestGraphReload({ type: "refresh" });
  }

  // Retry from the error state: go back to the spinner so a second failure is
  // distinguishable from the first, then re-run the same in-place reload.
  function handleRetry() {
    applyLoadOutcome({ state: "loading", error: null, refreshError: null });
    requestGraphReload({ type: "refresh" });
  }

  function handleIframeLoad() {
    // Snapshot the epoch for the bootstrap load, then push the theme. The
    // bootstrap fetch runs inside the guest, so arm the same dropped-reply
    // fallback here — without it a guest that never answers spins forever.
    loadEpochRef.current = dirtyEpochRef.current;
    sendTheme();
    sendShortcuts();
    armLoadFallback();
  }

  // 'reload' tracks the live option (toggling swaps the src → full reload);
  // 'in-place' reads the value frozen at frame mount so changes ride postMessage.
  const iframeSrc = boot.current
    ? graphIframeSrc({
        excludeSingleAuthors:
          HIDE_SINGLE_AUTHORS_STRATEGY === "reload" ? hideSingleAuthors : boot.current.exclude,
        api: GRAPH_API_TRANSPORT,
        mode: boot.current.mode,
        theme: boot.current.theme,
      })
    : null;

  return (
    <div className="w-full h-full flex flex-col">
      <div className="p-4 border-b border-border flex items-center gap-3">
        <h1 className="font-display text-[27px] font-semibold leading-tight tracking-[-0.015em] text-text">Knowledge Graph</h1>
        <span className="text-sm text-muted">
          {selectedSourceIds.length > 0
            ? `${selectedSourceIds.length} paper${selectedSourceIds.length !== 1 ? "s" : ""} selected; Ctrl/Cmd+click to add more`
            : "Click a node to open · Ctrl/Cmd+click to select"}
        </span>
        <div className="ml-auto flex items-center gap-4">
          {/* A reload that failed with a graph still drawn underneath. It says
              so here instead of covering that graph with the error card — the
              canvas the user panned and zoomed to is still valid, and the
              Refresh dot beside this is already flagged for the retry. */}
          {refreshError && (
            <span
              role="status"
              className="text-sm max-w-[32ch] truncate"
              style={{ color: "var(--color-danger)" }}
              title={refreshError}
            >
              Refresh failed: {refreshError}
            </span>
          )}
          <Button
            variant="ghost"
            size="sm"
            onClick={handleRefresh}
            disabled={refreshing}
            title={
              dirty
                ? "Graph data has changed since it was loaded. Click to refresh"
                : "Reload the graph from the latest data"
            }
          >
            {refreshing ? "Refreshing…" : "Refresh"}
            {dirty && !refreshing && (
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
      {/* The frame stays at full size underneath the overlay: cytoscape fits
          against its container, so shrinking or unmounting it while loading
          would put the graph back to the unframed-at-zoom-1 state. */}
      <div className="flex-1 relative" style={{ backgroundColor: "var(--color-bg)" }}>
        {/* One render without the frame: the effect above mounts it as soon as
            the route is entered. */}
        {iframeSrc && (
          <iframe
            ref={iframeRef}
            src={iframeSrc}
            className="absolute inset-0 border-0 w-full h-full"
            title="Paper knowledge graph"
            onLoad={handleIframeLoad}
          />
        )}
        {loadState !== "ready" && (
          <div
            className="absolute inset-0 overflow-y-auto flex items-center justify-center"
            style={{ backgroundColor: "var(--color-bg)" }}
          >
            {loadState === "loading" ? (
              <Spinner size={28} />
            ) : loadState === "empty" ? (
              <EmptyState
                icon={<Network size={28} strokeWidth={1.5} />}
                title="Nothing to graph yet"
                description="The knowledge graph is drawn from your library — import a few papers and they'll appear here, linked by their authors and tags."
                actionLabel="Go to Library"
                onAction={() => navigate("/library")}
              />
            ) : (
              <EmptyState
                icon={<AlertCircle size={28} strokeWidth={1.5} />}
                title="Couldn't load the graph"
                description={
                  loadError
                    ? `The graph data could not be fetched: ${loadError}`
                    : "The graph data could not be fetched."
                }
                actionLabel={refreshing ? "Retrying…" : "Retry"}
                onAction={handleRetry}
              />
            )}
          </div>
        )}
      </div>

      {selectedSourceIds.length > 0 && (
        <div
          className="shrink-0 flex items-center justify-between px-6 py-3 border-t border-border shadow-lg"
          style={{ backgroundColor: "var(--color-panel)" }}
        >
          <span className="text-sm font-medium text-text">
            {selectedSourceIds.length} selected
          </span>
          <div className="flex items-center gap-2">
            <Button variant="ghost" size="sm" onClick={handleClearSelection}>
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
                    addToProjectMutation.mutate({ projectId: project.id, sourceIds: selectedSourceIds })
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
