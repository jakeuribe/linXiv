import { useCallback, useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { useThemeStore } from "../stores/theme";
import { useUiStore } from "../stores/ui";
import { getColors } from "../lib/theme";
import { listProjects, addPaperToProject, createProject } from "../api/projects";
import { Spinner } from "../components/ui/spinner";
import { Button } from "../components/ui/button";
import { Dialog } from "../components/ui/dialog";
import { Input } from "../components/ui/input";

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

  const { data: projectsData, isLoading: projectsLoading } = useQuery({
    queryKey: ["projects"],
    queryFn: () => listProjects(),
    enabled: projectPickerOpen,
  });

  const addToProjectMutation = useMutation({
    mutationFn: async ({ projectId, sourceIds }: { projectId: number; sourceIds: string[] }) => {
      const results = await Promise.allSettled(
        sourceIds.map(id => addPaperToProject(projectId, id))
      );
      return sourceIds.filter((_, i) => results[i].status === "rejected");
    },
    onMutate: () => {
      setProjectPickerError(null);
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ["projects"] });
    },
    onSuccess: (failedIds, { sourceIds }) => {
      if (failedIds.length > 0) {
        setSelectedSourceIds(failedIds);
        setProjectPickerError(
          `${failedIds.length} of ${sourceIds.length} paper${sourceIds.length !== 1 ? "s" : ""} could not be added`
        );
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
    mutationFn: async ({ name, sourceIds }: { name: string; sourceIds: string[] }) => {
      const result = await createProject({ name });
      await Promise.allSettled(sourceIds.map((id) => addPaperToProject(result.project.id, id)));
    },
    onSuccess: () => {
      setNewProjectName("");
      setProjectPickerOpen(false);
      setProjectPickerError(null);
      setSelectedSourceIds([]);
      postToIframe({ type: "clear_selection" });
      queryClient.invalidateQueries({ queryKey: ["projects"] });
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
      } else if (e.data.type === "selection_changed" && Array.isArray(e.data.sourceIds)) {
        setSelectedSourceIds(e.data.sourceIds);
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

  // Toggling the single-author filter reloads the iframe (its src carries the
  // flag), which boots with an empty selection. Clear the parent's selection too
  // so the header count and "Add to project" target don't reference nodes the
  // reloaded graph no longer shows as selected.
  useEffect(() => {
    setSelectedSourceIds([]);
  }, [hideSingleAuthors]);

  function handleClearSelection() {
    setSelectedSourceIds([]);
    postToIframe({ type: "clear_selection" });
  }

  return (
    <div className="w-full h-full flex flex-col">
      <div className="p-4 border-b border-border flex items-center gap-3">
        <h1 className="text-lg font-semibold text-text">Knowledge Graph</h1>
        <span className="text-sm text-muted">
          {selectedSourceIds.length > 0
            ? `${selectedSourceIds.length} paper${selectedSourceIds.length !== 1 ? "s" : ""} selected — Ctrl/Cmd+click to add more`
            : "Click a node to open · Ctrl/Cmd+click to select"}
        </span>
        <label
          className="ml-auto flex items-center gap-2 text-sm text-muted cursor-pointer select-none"
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
      <iframe
        ref={iframeRef}
        src={hideSingleAuthors ? "/graph/graph.html?excludeSingleAuthors=1" : "/graph/graph.html"}
        className="flex-1 border-0 w-full"
        title="Paper knowledge graph"
        onLoad={sendTheme}
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
