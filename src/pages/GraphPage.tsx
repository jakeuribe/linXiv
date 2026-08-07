import { useCallback, useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { useThemeStore } from "../stores/theme";
import { useUiStore } from "../stores/ui";
import { getColors } from "../lib/theme";
import { listProjects, addPapers, createProjectWithPapers } from "../api/projects";
import { invalidateProjectMembershipQueries, partialFailureMessage } from "../lib/paperMutations";
import { getStats } from "../api/settings";
import { Spinner } from "../components/ui/spinner";
import { Button } from "../components/ui/button";
import { formSubmitOnCtrlEnter } from "../lib/submitShortcut";
import { Dialog } from "../components/ui/dialog";
import { Input } from "../components/ui/input";

// Per-option: 'in-place' applies via postMessage (iframe keeps its state);
// 'reload' lets the src track the live value so toggling re-bootstraps graph.js.
type ReloadStrategy = "in-place" | "reload";
const HIDE_SINGLE_AUTHORS_STRATEGY: ReloadStrategy = "in-place";

// Root query keys whose invalidation may change graph-relevant data.
const GRAPH_DIRTYING_KEYS = new Set([
  "stats", "papers", "paper", "projects", "project", "tags", "tag", "authors", "author",
]);

// How long to wait for a graph_loaded reply before clearing the spinner.
const REFRESH_FALLBACK_MS = 8000;

export default function GraphPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const preset = useThemeStore(s => s.preset);
  const mode = useThemeStore(s => s.mode);
  const overrides = useThemeStore(s => s.overrides);
  const overrideAlphas = useThemeStore(s => s.overrideAlphas);
  const hideSingleAuthors = useUiStore(s => s.hideSingleAuthors);
  const setHideSingleAuthors = useUiStore(s => s.setHideSingleAuthors);

  const [selectedSourceIds, setSelectedSourceIds] = useState<string[]>([]);
  const [projectPickerOpen, setProjectPickerOpen] = useState(false);
  const [projectPickerError, setProjectPickerError] = useState<string | null>(null);
  const [newProjectName, setNewProjectName] = useState("");
  const [dirty, setDirty] = useState(false);
  const [refreshing, setRefreshing] = useState(false);

  // Frozen at mount: drives the iframe src under the 'in-place' strategy.
  const initialExclude = useRef(hideSingleAuthors).current;
  // Last option value pushed to the iframe; the option effect bails when this
  // is unchanged.
  const appliedExclude = useRef(initialExclude);
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

  const addToProjectMutation = useMutation({
    mutationFn: addPapers,
    onMutate: () => {
      setProjectPickerError(null);
    },
    onSettled: () => {
      invalidateProjectMembershipQueries(queryClient);
    },
    onSuccess: (failedIds, { sourceIds }) => {
      if (failedIds.length > 0) {
        // Re-select only the failures so a retry can't re-add the rest.
        setSelectedSourceIds(failedIds);
        setProjectPickerError(partialFailureMessage(failedIds.length, sourceIds.length));
      } else {
        setProjectPickerOpen(false);
        setProjectPickerError(null);
        setSelectedSourceIds([]);
        postToIframe({ type: "clear_selection" });
      }
    },
    onError: (err) => {
      setProjectPickerError(err instanceof Error ? err.message : "Failed to add papers to project");
    },
  });

  const createProjectMutation = useMutation({
    mutationFn: createProjectWithPapers,
    // Invalidate in onSettled, not onSuccess: the project may have been
    // created even when the mutation rejects (e.g. a paper-add request fails).
    onSettled: () => {
      invalidateProjectMembershipQueries(queryClient);
    },
    onSuccess: (failedIds) => {
      // The project exists either way — clear the name so a retry can't
      // create a duplicate.
      setNewProjectName("");
      if (failedIds.length > 0) {
        setSelectedSourceIds(failedIds);
        setProjectPickerError(
          `Project created, but ${failedIds.length} paper${failedIds.length !== 1 ? "s" : ""} could not be added`
        );
        return;
      }
      setProjectPickerOpen(false);
      setProjectPickerError(null);
      setSelectedSourceIds([]);
      postToIframe({ type: "clear_selection" });
    },
    onError: (err) => {
      setProjectPickerError(err instanceof Error ? err.message : "Failed to create project");
    },
  });

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
      }
    }
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, [navigate]);

  const sendTheme = useCallback(() => {
    const iframe = iframeRef.current;
    if (!iframe?.contentWindow) return;
    const colors = getColors(preset, mode, overrides, overrideAlphas);
    iframe.contentWindow.postMessage({ type: "theme_update", colors }, window.location.origin);
  }, [preset, mode, overrides, overrideAlphas]);

  useEffect(() => {
    sendTheme();
  }, [sendTheme]);

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
  useEffect(() => {
    const unsubscribe = queryClient.getQueryCache().subscribe((event) => {
      if (event.type !== "updated" || event.action.type !== "invalidate") return;
      const root = event.query.queryKey[0];
      if (typeof root === "string" && GRAPH_DIRTYING_KEYS.has(root)) {
        dirtyEpochRef.current++;
        setDirty(true);
      }
    });
    return unsubscribe;
  }, [queryClient]);

  useEffect(() => () => {
    if (refreshTimerRef.current !== null) clearTimeout(refreshTimerRef.current);
  }, []);

  function handleClearSelection() {
    setSelectedSourceIds([]);
    postToIframe({ type: "clear_selection" });
  }

  // Drive an in-place reload (refresh or option toggle): snapshot the epoch, show
  // the spinner, and arm a fallback for a dropped graph_loaded reply.
  function requestGraphReload(message: object) {
    setRefreshing(true);
    loadEpochRef.current = dirtyEpochRef.current;
    postToIframe(message);
    if (refreshTimerRef.current !== null) clearTimeout(refreshTimerRef.current);
    refreshTimerRef.current = window.setTimeout(() => {
      refreshTimerRef.current = null;
      setRefreshing(false);
      setDirty(true);
    }, REFRESH_FALLBACK_MS);
  }

  function handleRefresh() {
    requestGraphReload({ type: "refresh" });
  }

  function handleIframeLoad() {
    // Snapshot the epoch for the bootstrap load, then push the theme.
    loadEpochRef.current = dirtyEpochRef.current;
    sendTheme();
  }

  // 'reload' tracks the live option (toggling swaps the src → full reload);
  // 'in-place' freezes it at mount so changes ride postMessage instead.
  const srcExclude =
    HIDE_SINGLE_AUTHORS_STRATEGY === "reload" ? hideSingleAuthors : initialExclude;
  const iframeSrc = srcExclude
    ? "/graph/graph.html?excludeSingleAuthors=1"
    : "/graph/graph.html";

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
            title="Drop authors linked to only one paper to declutter the graph"
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
      <iframe
        ref={iframeRef}
        src={iframeSrc}
        className="flex-1 border-0 w-full"
        title="Paper knowledge graph"
        onLoad={handleIframeLoad}
      />

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
