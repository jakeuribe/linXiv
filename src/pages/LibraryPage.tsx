import { useState, useRef, useMemo, useDeferredValue, useCallback } from "react";
import { useNavigate } from "react-router";
import { useQuery, useMutation, useQueryClient, keepPreviousData } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import { Upload, FileText, SearchX, FilterX } from "lucide-react";
import { listPapers, deletePaper, searchLibrary } from "../api/papers";
import type { PaperSort } from "../api/papers";
import { listProjects } from "../api/projects";
import {
  invalidatePaperQueries,
  addToProjectMutationOptions,
  createProjectMutationOptions,
} from "../lib/paperMutations";
import { useSelectionStore } from "../stores/selection";
import { useLibraryStore } from "../stores/library";
import type { LibraryFilterMode as FilterMode } from "../stores/library";
import type { Paper } from "../types/api";
import { Spinner } from "../components/ui/spinner";
import { Input } from "../components/ui/input";
import { OptionSelect } from "../components/ui/select";
import { formSubmitOnCtrlEnter } from "../lib/submitShortcut";
import { Button } from "../components/ui/button";
import { Dialog } from "../components/ui/dialog";
import { PaperCard } from "../components/papers/PaperCard";
import { SelectionBar } from "../components/papers/SelectionBar";
import { ImportDialog } from "../components/import/ImportDialog";
import { EmptyState } from "../components/ui/empty-state";

const PAPER_FETCH_LIMIT = 5000;
const VIRTUALIZER_ESTIMATE_HEIGHT = 120;
const VIRTUALIZER_OVERSCAN = 5;
const ROW_GAP_PX = "12px";
const TRASH_RETENTION_DAYS = 30;

const FILTER_LABELS: { mode: FilterMode; label: string }[] = [
  { mode: "all", label: "All" },
  { mode: "has_pdf", label: "Has PDF" },
  { mode: "no_pdf", label: "No PDF" },
];

const SORT_OPTIONS: { value: PaperSort; label: string }[] = [
  { value: "published_desc", label: "Newest first" },
  { value: "published_asc", label: "Oldest first" },
  { value: "added_desc", label: "Recently added" },
  { value: "added_asc", label: "First added" },
  { value: "title_asc", label: "Title A–Z" },
  { value: "title_desc", label: "Title Z–A" },
];

function matchesPaper(paper: Paper, query: string): boolean {
  if (!query.trim()) return true;
  const q = query.toLowerCase();
  if (paper.title.toLowerCase().includes(q)) return true;
  if (paper.summary?.toLowerCase().includes(q)) return true;
  const authors = paper.authors;
  return authors.some((a) => a.toLowerCase().includes(q));
}

export default function LibraryPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();

  const search = useLibraryStore((s) => s.search);
  const setSearch = useLibraryStore((s) => s.setSearch);
  const deferredSearch = useDeferredValue(search);
  const trimmedSearch = deferredSearch.trim();
  const ftsEnabled = trimmedSearch.length >= 3;
  const filterMode = useLibraryStore((s) => s.filterMode);
  const setFilterMode = useLibraryStore((s) => s.setFilterMode);
  const sort = useLibraryStore((s) => s.sort);
  const setSort = useLibraryStore((s) => s.setSort);
  const [projectPickerOpen, setProjectPickerOpen] = useState(false);
  const [projectPickerError, setProjectPickerError] = useState<string | null>(null);
  const [newProjectName, setNewProjectName] = useState("");
  const [importOpen, setImportOpen] = useState(false);
  const [pendingDeleteIds, setPendingDeleteIds] = useState<string[]>([]);
  const [deleteError, setDeleteError] = useState<string | null>(null);

  const selectedIds = useSelectionStore((s) => s.selectedIds);
  const clear = useSelectionStore((s) => s.clear);
  const selectAll = useSelectionStore((s) => s.selectAll);

  const scrollRef = useRef<HTMLDivElement>(null);

  const {
    data: papersData,
    isLoading,
    isFetching: papersFetching,
    error,
  } = useQuery({
    queryKey: ["papers", "list", sort],
    queryFn: () => listPapers(PAPER_FETCH_LIMIT, 0, sort),
    // The previous order stays on screen while the re-sorted page loads — so the
    // header spinner is the only cue the list is being refetched.
    placeholderData: keepPreviousData,
  });

  const {
    data: projectsData,
    isLoading: projectsLoading,
  } = useQuery({
    queryKey: ["projects"],
    queryFn: () => listProjects(),
    enabled: projectPickerOpen,
  });

  const {
    data: ftsData,
    isFetching: ftsFetching,
    isError: ftsError,
  } = useQuery({
    queryKey: ["papers", "search", trimmedSearch],
    queryFn: () => searchLibrary(trimmedSearch),
    enabled: ftsEnabled,
    staleTime: 30_000,
  });

  const deleteMutation = useMutation({
    mutationFn: async (ids: string[]) => {
      for (const id of ids) {
        await deletePaper(id);
      }
    },
    onMutate: () => {
      setDeleteError(null);
    },
    onSettled: () => {
      invalidatePaperQueries(queryClient);
    },
    onSuccess: () => {
      clear();
    },
    onError: (err) => {
      setDeleteError(
        err instanceof Error ? err.message : "Failed to delete papers"
      );
    },
  });

  const projectPickerUi = {
    setError: setProjectPickerError,
    selectFailures: selectAll,
    onDone: () => {
      setProjectPickerOpen(false);
      clear();
    },
    clearName: () => setNewProjectName(""),
  };

  const addToProjectMutation = useMutation(
    addToProjectMutationOptions(queryClient, projectPickerUi)
  );

  const createProjectMutation = useMutation(
    createProjectMutationOptions(queryClient, projectPickerUi)
  );

  const allPapers = papersData?.papers ?? [];

  const filtered = useMemo(() => {
    const ftsPapers = ftsEnabled ? (ftsData?.papers ?? []) : [];
    const ftsIds = new Set(ftsPapers.map((p) => p.source_id));

    const seen = new Set<string>();
    const result: Paper[] = [];

    for (const paper of allPapers) {
      if (!matchesPaper(paper, deferredSearch) && !ftsIds.has(paper.source_id)) continue;
      if (filterMode === "has_pdf" && !paper.has_pdf) continue;
      if (filterMode === "no_pdf" && paper.has_pdf) continue;
      seen.add(paper.source_id);
      result.push(paper);
    }

    // FTS results not in the loaded window (for libraries exceeding PAPER_FETCH_LIMIT)
    for (const paper of ftsPapers) {
      if (seen.has(paper.source_id)) continue;
      if (filterMode === "has_pdf" && !paper.has_pdf) continue;
      if (filterMode === "no_pdf" && paper.has_pdf) continue;
      result.push(paper);
    }

    return result;
  }, [allPapers, deferredSearch, filterMode, ftsEnabled, ftsData]);

  const virtualizer = useVirtualizer({
    count: filtered.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => VIRTUALIZER_ESTIMATE_HEIGHT,
    getItemKey: (i) => filtered[i].source_id,
    overscan: VIRTUALIZER_OVERSCAN,
  });

  const handleNavigate = useCallback(
    (sfk: number) => navigate(`/library/${sfk}`),
    [navigate]
  );

  // Re-sorting keeps the row count identical, so the browser never clamps
  // scrollTop — without this the user stays at row 800 of a list that now holds
  // entirely different papers there.
  const handleSortChange = useCallback(
    (next: PaperSort) => {
      setSort(next);
      scrollRef.current?.scrollTo({ top: 0 });
    },
    [setSort]
  );

  function handleDeleteRequest() {
    if (deleteMutation.isPending) return;
    const visibleIds = new Set(filtered.map((p) => p.source_id));
    const ids = Array.from(selectedIds).filter((id) => visibleIds.has(id));
    if (ids.length === 0) return;
    setPendingDeleteIds(ids);
  }

  function handleDeleteConfirm() {
    if (pendingDeleteIds.length > 0) deleteMutation.mutate(pendingDeleteIds);
    setPendingDeleteIds([]);
  }

  function handleAddToProject(projectId: number) {
    if (addToProjectMutation.isPending) return;
    const ids = Array.from(selectedIds);
    if (ids.length === 0) return; // nothing to add
    addToProjectMutation.mutate({ projectId, sourceIds: ids });
  }

  const paperCountLabel = useMemo(() => {
    const total = allPapers.length;
    const shown = filtered.length;
    const limitReached = total >= PAPER_FETCH_LIMIT;
    if (shown < total) {
      const totalStr = `${total}${limitReached ? "+" : ""}`;
      return `${shown} of ${totalStr} paper${shown !== 1 ? "s" : ""}`;
    }
    const overflowed = shown > total || limitReached;
    return `${shown}${overflowed ? "+" : ""} paper${shown !== 1 ? "s" : ""}`;
  }, [allPapers.length, filtered.length]);

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-full">
        <Spinner size={28} />
      </div>
    );
  }

  if (error) {
    return (
      <div className="flex items-center justify-center h-full">
        <p className="text-sm" style={{ color: "var(--color-danger)" }}>
          {error instanceof Error ? error.message : "Failed to load papers"}
        </p>
      </div>
    );
  }

  return (
    <div className="flex flex-col h-full">
      {/* Header block */}
      <div className="shrink-0 border-b border-border" style={{ padding: "22px 30px 16px" }}>
        <div className="flex items-end gap-3.5">
          <div className="min-w-0">
            <h1 className="font-display text-[27px] font-semibold leading-tight tracking-[-0.015em] text-text whitespace-nowrap">
              Library
            </h1>
            <p className="text-muted" style={{ fontSize: 13, marginTop: 7 }}>
              {paperCountLabel}
              {ftsEnabled && ftsError && (
                <span style={{ color: "var(--color-danger)" }} className="ml-2 text-xs">
                  search error
                </span>
              )}
            </p>
          </div>
          <div className="flex-1" />
          <span className="flex items-center gap-1.5 text-muted text-sm shrink-0">
            {((ftsEnabled && ftsFetching) || papersFetching) && <Spinner size={12} />}
          </span>
          <Input
            placeholder="Search by title, abstract, full text, or notes…"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            className="max-w-xs"
          />
          <Button variant="primary" size="sm" onClick={() => setImportOpen(true)}>
            <Upload size={13} className="mr-1" />Import
          </Button>
        </div>
        {deleteError && (
          <p className="text-sm mt-2" style={{ color: "var(--color-danger)" }}>
            {deleteError}
          </p>
        )}
        <div className="flex items-center gap-1.75 flex-wrap" style={{ marginTop: 16 }}>
          {FILTER_LABELS.map(({ mode, label }) => (
            <button
              type="button"
              key={mode}
              onClick={() => setFilterMode(mode)}
              aria-pressed={filterMode === mode}
              className={[
                "px-3 py-1 rounded-full text-xs font-medium transition-colors border",
                filterMode === mode
                  ? "border-[var(--color-accent)] text-[var(--color-accent)] bg-[color-mix(in_srgb,var(--color-accent)_12%,transparent)]"
                  : "border-border text-muted hover:border-[var(--color-muted)]",
              ].join(" ")}
            >
              {label}
            </button>
          ))}
          <div className="flex-1" />
          <OptionSelect
            aria-label="Sort papers by"
            size="sm"
            value={sort}
            onChange={handleSortChange}
            options={SORT_OPTIONS}
          />
        </div>
      </div>

      <div
        ref={scrollRef}
        className={`flex-1 overflow-y-auto px-7.5 pt-4.5 ${selectedIds.size > 0 ? "pb-20" : "pb-10"}`}
      >
        {filtered.length === 0 ? (
          allPapers.length === 0 ? (
            <EmptyState
              icon={<FileText size={28} strokeWidth={1.5} />}
              title="Your library is empty"
              description="Import papers from arXiv to start building your reading library."
              actionLabel="Import from arXiv"
              onAction={() => setImportOpen(true)}
            />
          ) : trimmedSearch.length > 0 && filterMode !== "all" ? (
            <EmptyState
              icon={<SearchX size={28} strokeWidth={1.5} />}
              title="No matching papers"
              description={`Nothing matches “${trimmedSearch}” with the current filter. Clear both to see everything.`}
              actionLabel="Clear all"
              onAction={() => {
                setSearch("");
                setFilterMode("all");
              }}
            />
          ) : trimmedSearch.length > 0 ? (
            <EmptyState
              icon={<SearchX size={28} strokeWidth={1.5} />}
              title="No matching papers"
              description={`Nothing in your library matches “${trimmedSearch}”. Try a different term or clear the search.`}
              actionLabel="Clear search"
              onAction={() => setSearch("")}
            />
          ) : (
            <EmptyState
              icon={<FilterX size={28} strokeWidth={1.5} />}
              title="No papers match this filter"
              description="No papers in your library match the current filter. Reset it to see everything."
              actionLabel="Clear filters"
              onAction={() => setFilterMode("all")}
            />
          )
        ) : (
          <div
            style={{
              height: virtualizer.getTotalSize(),
              width: "100%",
              position: "relative",
            }}
          >
            {virtualizer.getVirtualItems().map((vItem) => (
              <div
                key={vItem.key}
                data-index={vItem.index}
                ref={virtualizer.measureElement}
                style={{
                  position: "absolute",
                  top: 0,
                  left: 0,
                  right: 0,
                  transform: `translateY(${vItem.start}px)`,
                  paddingBottom: ROW_GAP_PX,
                }}
              >
                <PaperCard
                  paper={filtered[vItem.index]}
                  showCheckbox
                  onNavigate={handleNavigate}
                />
              </div>
            ))}
          </div>
        )}
      </div>

      {/* Selection bar */}
      <SelectionBar
        count={selectedIds.size}
        onAddToProject={() => setProjectPickerOpen(true)}
        onDelete={handleDeleteRequest}
        onClear={clear}
      />

      {/* Delete confirmation dialog */}
      <Dialog
        open={pendingDeleteIds.length > 0}
        onClose={() => setPendingDeleteIds([])}
        title="Delete Papers"
      >
        <div className="space-y-4">
          <p className="text-sm text-text">
            Send {pendingDeleteIds.length} paper{pendingDeleteIds.length !== 1 ? "s" : ""} to trash?
            They can be restored from Settings within {TRASH_RETENTION_DAYS} days.
          </p>
          <div className="flex justify-end gap-2">
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setPendingDeleteIds([])}
            >
              Cancel
            </Button>
            <Button
              variant="danger"
              size="sm"
              onClick={handleDeleteConfirm}
              disabled={deleteMutation.isPending}
            >
              {deleteMutation.isPending ? "Deleting…" : "Delete"}
            </Button>
          </div>
        </div>
      </Dialog>

      {/* Import dialog */}
      <ImportDialog
        open={importOpen}
        onClose={() => setImportOpen(false)}
        onDone={(newProjectIds) => {
          queryClient.invalidateQueries({ queryKey: ["papers"] });
          queryClient.invalidateQueries({ queryKey: ["stats"] });
          if (newProjectIds.length === 1) {
            navigate(`/projects/${newProjectIds[0]}`);
          }
        }}
      />

      {/* Add to project dialog */}
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
                  if (name) createProjectMutation.mutate({ name, sourceIds: [...selectedIds] });
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
                  onClick={() => handleAddToProject(project.id)}
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
