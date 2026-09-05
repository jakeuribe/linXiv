import { useCallback, useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router";
import { useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { BookMarked } from "lucide-react";
import { listProjects, createProject, archiveProject } from "../api/projects";
import type { Paper, Project } from "../types/api";
import { showContextMenu } from "../lib/contextMenu";
import { listProjectPapers } from "../api/papers";
import { ProjectCard } from "../components/projects/ProjectCard";
import { PaperCard } from "../components/papers/PaperCard";
import { Button } from "../components/ui/button";
import { Dialog } from "../components/ui/dialog";
import { EmptyState } from "../components/ui/empty-state";
import { Input } from "../components/ui/input";
import { Segmented } from "../components/ui/segmented";
import { Spinner } from "../components/ui/spinner";
import { StatusButton, useSetReadingStatus } from "../components/reading/StatusButton";
import {
  READING_LIST_TAG,
  isReadingListProject,
  queueOf,
} from "../lib/readingStatus";
import { invalidateProjectMutationQueries } from "../lib/paperMutations";
import {
  READING_STATUS_QUERY_KEY,
  fetchReadingStatuses,
} from "../api/readingStatus";
import { listReceived, sharingAvailable } from "../api/share";
import { receivedShareRole } from "../lib/shareRole";
import { errText } from "../lib/errText";

function NewReadingListDialog({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const queryClient = useQueryClient();
  const [name, setName] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (open) setError(null);
  }, [open]);

  function handleClose() {
    setName("");
    setError(null);
    onClose();
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (submitting) return;
    if (!name.trim()) return;
    setSubmitting(true);
    setError(null);
    try {
      await createProject({
        name: name.trim(),
        project_tags: [READING_LIST_TAG],
      });
      await invalidateProjectMutationQueries(queryClient);
      handleClose();
    } catch (err) {
      setError(
        errText(err, "Failed to create reading list")
      );
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Dialog open={open} onClose={handleClose} title="New Reading List">
      <form onSubmit={handleSubmit} className="flex flex-col gap-4">
        <div className="flex flex-col gap-1.5">
          <label
            htmlFor="rl-name"
            className="text-xs font-medium"
            style={{ color: "var(--color-muted)" }}
          >
            Name <span style={{ color: "var(--color-danger)" }}>*</span>
          </label>
          <Input
            id="rl-name"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="Reading list name"
            required
            autoFocus
          />
        </div>
        {error && (
          <p className="text-xs" style={{ color: "var(--color-danger)" }}>
            {error}
          </p>
        )}
        <div className="flex justify-end gap-2 pt-1">
          <Button type="button" variant="muted" onClick={handleClose} disabled={submitting}>
            Cancel
          </Button>
          <Button type="submit" disabled={!name.trim() || submitting}>
            {submitting ? <Spinner size={14} /> : "Create"}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}

export default function ReadingListsPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [view, setView] = useState<"lists" | "queue">("lists");
  const [dialogOpen, setDialogOpen] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const setStatus = useSetReadingStatus();

  // Same archive call ProjectDetailPage's ··· menu makes (a reading list IS a
  // project); archiving hides the list and its queue rows.
  async function handleArchive(project: Project) {
    setActionError(null);
    try {
      await archiveProject(project.id);
      await invalidateProjectMutationQueries(queryClient);
    } catch (err) {
      setActionError(errText(err, "Failed to archive reading list"));
    }
  }

  function handleListContextMenu(e: React.MouseEvent, project: Project) {
    // §7 viewer read-only: no Archive on a viewer-role shared list — same
    // gating as ProjectDetailPage's edit affordances.
    const viewer = receivedShareRole(project, receivedShares) === "viewer";
    showContextMenu(e, [
      { text: "Open", action: () => navigate(`/projects/${project.id}`) },
      ...(viewer ? [] : [{ text: "Archive", action: () => void handleArchive(project) }]),
    ]);
  }

  // Depends on the stable `mutate` fn, not the per-render mutation object, so
  // the callback keeps its identity and PaperCard's memo actually skips.
  const setStatusMutate = setStatus.mutate;
  const handleQueueContextMenu = useCallback(
    (e: React.MouseEvent, paper: Paper) => {
      const mark = (status: "reading" | "read" | undefined) =>
        setStatusMutate({ sourceId: paper.source_id, status });
      showContextMenu(e, [
        { text: "Open", action: () => navigate(`/library/${paper.source_fk}`) },
        "separator",
        { text: "Mark Reading", action: () => mark("reading") },
        { text: "Mark Read", action: () => mark("read") },
        { text: "Mark Unread", action: () => mark(undefined) },
      ]);
    },
    [navigate, setStatusMutate]
  );
  const { data: statuses = {} } = useQuery({
    queryKey: READING_STATUS_QUERY_KEY,
    queryFn: fetchReadingStatuses,
  });

  const { data: projectsData, isLoading: projectsLoading, isError: projectsError, error: projectsErrorMsg } = useQuery({
    queryKey: ["projects", "active"],
    queryFn: () => listProjects("active"),
  });

  const readingLists = useMemo(() => {
    return (projectsData?.projects ?? []).filter(isReadingListProject);
  }, [projectsData]);

  const { data: receivedShares } = useQuery({
    queryKey: ["share", "received"],
    queryFn: listReceived,
    enabled: sharingAvailable && readingLists.some((p) => p.share_id),
  });

  // One server-filtered fetch per reading list — membership is decided in SQL,
  // so a >200-paper library no longer truncates the queue. Keys match the
  // ["papers", ...] prefix that project-membership mutations invalidate.
  const { papers: listPapersFlat, isLoading: papersLoading, isError: papersError, error: papersErrorMsg } = useQueries({
    queries: readingLists.map((p) => ({
      queryKey: ["papers", { project: p.id }],
      queryFn: () => listProjectPapers(p.id),
    })),
    combine: (results) => ({
      papers: results.flatMap((r) => r.data?.papers ?? []),
      isLoading: results.some((r) => r.isLoading),
      isError: results.some((r) => r.isError),
      error: results.find((r) => r.error)?.error ?? null,
    }),
  });

  const queue = useMemo(() => {
    // Dedupe: a paper on several reading lists arrives once per list.
    const bySid = new Map(listPapersFlat.map((p) => [p.source_id, p]));
    return queueOf([...bySid.values()], new Set(bySid.keys()), statuses);
  }, [listPapersFlat, statuses]);

  const loading = projectsLoading || papersLoading;
  const isError = projectsError || papersError;
  const errorMsg = projectsErrorMsg || papersErrorMsg;

  return (
    <div className="flex flex-col gap-6 p-8 h-full overflow-y-auto">
      <div className="flex items-center justify-between">
        <h1 className="font-display text-[27px] font-semibold leading-tight tracking-[-0.015em] text-text">
          Reading Lists
        </h1>
        <Button onClick={() => setDialogOpen(true)}>New Reading List</Button>
      </div>

      <Segmented
        aria-label="Reading view"
        value={view}
        onChange={setView}
        options={[
          { value: "lists", label: "Lists" },
          { value: "queue", label: `Queue${queue.length ? ` (${queue.length})` : ""}` },
        ]}
      />

      {loading && (
        <div className="flex-1 flex items-center justify-center">
          <Spinner size={28} />
        </div>
      )}

      {actionError && (
        <p className="text-xs" style={{ color: "var(--color-danger)" }}>
          {actionError}
        </p>
      )}

      {isError && (
        <div
          className="rounded-lg border p-4 text-sm"
          style={{
            borderColor: "var(--color-danger)",
            color: "var(--color-danger)",
            backgroundColor: "var(--color-panel)",
          }}
        >
          Failed to load reading lists:{" "}
          {errText(errorMsg, "Unknown error")}
        </div>
      )}

      {!loading && !isError && view === "lists" && readingLists.length === 0 && (
        <EmptyState
          icon={<BookMarked size={28} />}
          title="No reading lists"
          description="A reading list is a project tagged for reading. Create one, add papers to it, then track them in the queue."
          actionLabel="New Reading List"
          onAction={() => setDialogOpen(true)}
        />
      )}

      {!loading && !isError && view === "lists" && readingLists.length > 0 && (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
          {readingLists.map((project) => (
            <ProjectCard
              key={project.id}
              project={{
                ...project,
                project_tags: project.project_tags.filter(
                  (t) => t.toLowerCase() !== READING_LIST_TAG
                ),
              }}
              onClick={() => navigate(`/projects/${project.id}`)}
              onContextMenu={(e) => handleListContextMenu(e, project)}
            />
          ))}
        </div>
      )}

      {!loading && !isError && view === "queue" && queue.length === 0 && (
        <EmptyState
          icon={<BookMarked size={28} />}
          title="Queue is empty"
          description="Papers on your reading lists that you haven't finished show up here. Mark a paper read to clear it from the queue."
        />
      )}

      {!loading && !isError && view === "queue" && queue.length > 0 && (
        <div className="flex flex-col gap-3">
          {queue.map((paper) => (
            <div key={paper.source_id} className="flex items-start gap-3">
              <div className="flex-1 min-w-0">
                <PaperCard
                  paper={paper}
                  onNavigate={(sfk) => navigate(`/library/${sfk}`)}
                  onContextMenu={handleQueueContextMenu}
                />
              </div>
              <StatusButton sourceId={paper.source_id} />
            </div>
          ))}
        </div>
      )}

      <NewReadingListDialog
        open={dialogOpen}
        onClose={() => setDialogOpen(false)}
      />
    </div>
  );
}
