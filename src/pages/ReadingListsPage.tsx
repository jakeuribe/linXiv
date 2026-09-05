import { useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router";
import { useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { BookMarked } from "lucide-react";
import { listProjects, createProject } from "../api/projects";
import { listProjectPapers } from "../api/papers";
import { ProjectCard } from "../components/projects/ProjectCard";
import { PaperCard } from "../components/papers/PaperCard";
import { Button } from "../components/ui/button";
import { Dialog } from "../components/ui/dialog";
import { EmptyState } from "../components/ui/empty-state";
import { Input } from "../components/ui/input";
import { Segmented } from "../components/ui/segmented";
import { Spinner } from "../components/ui/spinner";
import { StatusButton } from "../components/reading/StatusButton";
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
  const [view, setView] = useState<"lists" | "queue">("lists");
  const [dialogOpen, setDialogOpen] = useState(false);
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
