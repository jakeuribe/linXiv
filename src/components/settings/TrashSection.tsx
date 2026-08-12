import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  listTrash,
  restorePaper,
  hardDeletePaper,
  restoreProject,
  hardDeleteProject,
  type TrashedPaper,
  type TrashedProject,
} from "../../api/trash";
import { removeFromAllProjects } from "../../api/papers";
import { useReadingStatusStore } from "../../stores/readingStatus";
import {
  invalidatePaperQueries,
  invalidateProjectMembershipQueries,
  invalidateProjectMutationQueries,
  forgetPurgedPapers,
} from "../../lib/paperMutations";
import { Button } from "../ui/button";
import { Spinner } from "../ui/spinner";
import { Dialog } from "../ui/dialog";
import { SettingGroup, SettingGroupLabel } from "./SettingRow";
import { useConfirmWithTimeout } from "../../hooks/useConfirmWithTimeout";

type ProjectPrompt = {
  paperTitle: string;
  sourceFk: number;
  projectFks: number[];
};

function KeepInProjectsDialog({
  prompt,
  removing,
  removeError,
  onKeep,
  onRemove,
}: {
  prompt: ProjectPrompt | null;
  removing: boolean;
  removeError: string | null;
  onKeep: () => void;
  onRemove: () => void;
}) {
  const count = prompt?.projectFks.length ?? 0;
  return (
    <Dialog open={prompt !== null} onClose={onKeep} title="Project memberships">
      {prompt && (
        <>
          <p className="text-sm text-text mb-1">
            <span className="font-medium">{prompt.paperTitle}</span> was restored.
          </p>
          <p className="text-sm text-muted mb-4">
            It belonged to {count} project{count !== 1 ? "s" : ""} before deletion.
            Keep those memberships?
          </p>
          {removeError && (
            <p className="text-xs text-danger mb-4">{removeError}</p>
          )}
          <div className="flex justify-end gap-2">
            <Button variant="ghost" size="sm" onClick={onRemove} disabled={removing}>
              {removing ? <><Spinner size={14} /> Removing…</> : "Remove from all projects"}
            </Button>
            <Button variant="primary" size="sm" onClick={onKeep} disabled={removing}>
              Keep memberships
            </Button>
          </div>
        </>
      )}
    </Dialog>
  );
}

function TrashActions({
  onRestore,
  onDeleteClick,
  disarm,
  isPending,
  restoring,
  deleting,
  confirm,
}: {
  onRestore: () => void;
  onDeleteClick: () => void;
  disarm: () => void;
  isPending: boolean;
  restoring: boolean;
  deleting: boolean;
  confirm: boolean;
}) {
  return (
    <div className="flex-shrink-0 flex gap-2">
      <Button variant="ghost" size="sm" onClick={onRestore} disabled={isPending}>
        {restoring ? <Spinner size={14} /> : "Restore"}
      </Button>
      <Button
        variant="danger"
        size="sm"
        onClick={onDeleteClick}
        onBlur={disarm}
        disabled={isPending}
      >
        {deleting ? <Spinner size={14} /> : (confirm ? "Confirm?" : "Delete forever")}
      </Button>
    </div>
  );
}

function TrashRow({
  paper,
  onRestore,
  onDelete,
  restoring,
  deleting,
}: {
  paper: TrashedPaper;
  onRestore: () => void;
  onDelete: () => void;
  restoring: boolean;
  deleting: boolean;
}) {
  const { confirm, arm, disarm } = useConfirmWithTimeout();
  const isPending = restoring || deleting;

  function handleDeleteClick() {
    if (confirm) { disarm(); onDelete(); } else { arm(); }
  }

  const authorLine =
    paper.authors && paper.authors.length > 0
      ? paper.authors.length > 1
        ? `${paper.authors[0]} et al.`
        : paper.authors[0]
      : null;

  return (
    <div className="flex items-center justify-between py-3 border-b border-border last:border-0">
      <div className="flex-1 min-w-0 mr-4">
        <p className="text-sm font-medium text-text truncate">{paper.title}</p>
        {authorLine && <p className="text-xs text-muted mt-0.5">{authorLine}</p>}
      </div>
      <TrashActions
        onRestore={onRestore}
        onDeleteClick={handleDeleteClick}
        disarm={disarm}
        isPending={isPending}
        restoring={restoring}
        deleting={deleting}
        confirm={confirm}
      />
    </div>
  );
}

function ProjectTrashRow({
  project,
  onRestore,
  onDelete,
  restoring,
  deleting,
}: {
  project: TrashedProject;
  onRestore: () => void;
  onDelete: () => void;
  restoring: boolean;
  deleting: boolean;
}) {
  const { confirm, arm, disarm } = useConfirmWithTimeout();
  const isPending = restoring || deleting;

  function handleDeleteClick() {
    if (confirm) { disarm(); onDelete(); } else { arm(); }
  }

  return (
    <div className="flex items-center justify-between py-3 border-b border-border last:border-0">
      <div className="flex-1 min-w-0 mr-4">
        <p className="text-sm font-medium text-text truncate">{project.name}</p>
        <p className="text-xs text-muted mt-0.5">
          {project.paper_count} paper{project.paper_count !== 1 ? "s" : ""}
        </p>
      </div>
      <TrashActions
        onRestore={onRestore}
        onDeleteClick={handleDeleteClick}
        disarm={disarm}
        isPending={isPending}
        restoring={restoring}
        deleting={deleting}
        confirm={confirm}
      />
    </div>
  );
}

export function TrashSection() {
  const qc = useQueryClient();

  const { data, isLoading } = useQuery({
    queryKey: ["trash"],
    queryFn: listTrash,
    staleTime: 0,
  });

  const papers = data?.papers ?? [];
  const projects = data?.projects ?? [];
  const total = papers.length + projects.length;

  const [restoringId, setRestoringId] = useState<string | null>(null);
  const [deletingId, setDeletingId] = useState<string | null>(null);
  const [restoringProjectId, setRestoringProjectId] = useState<number | null>(null);
  const [deletingProjectId, setDeletingProjectId] = useState<number | null>(null);
  const [projectPrompt, setProjectPrompt] = useState<ProjectPrompt | null>(null);
  const [removing, setRemoving] = useState(false);
  const [removeError, setRemoveError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  function closeProjectPrompt() {
    if (removing) return;
    setRemoveError(null);
    setProjectPrompt(null);
  }

  async function handleRestore(paper: TrashedPaper) {
    if (removing) return;
    setRestoringId(paper.source_id);
    setActionError(null);
    try {
      const result = await restorePaper(paper.source_id);
      await invalidatePaperQueries(qc);
      const projectFks = result.project_fks ?? [];
      if (projectFks.length > 0) {
        setProjectPrompt({
          paperTitle: paper.title,
          sourceFk: paper.source_fk,
          projectFks,
        });
      }
    } catch (e) {
      console.error(e);
      setActionError(`Failed to restore "${paper.title}".`);
    } finally {
      setRestoringId(null);
    }
  }

  async function handleRemoveFromProjects() {
    if (!projectPrompt) return;
    setRemoving(true);
    setRemoveError(null);
    try {
      await removeFromAllProjects(projectPrompt.sourceFk);
      await invalidateProjectMembershipQueries(qc);
      setProjectPrompt(null);
    } catch (e) {
      console.error(e);
      setRemoveError("Failed to remove from projects.");
    } finally {
      setRemoving(false);
    }
  }

  async function handleDelete(sourceId: string) {
    setDeletingId(sourceId);
    setActionError(null);
    try {
      await hardDeletePaper(sourceId);
      // Drop the persisted reading status so a re-saved paper reusing this
      // source_id starts fresh. Soft delete keeps it: trash+restore round-trips.
      forgetPurgedPapers([sourceId], useReadingStatusStore.getState());
      await invalidatePaperQueries(qc);
    } catch (e) {
      console.error(e);
      setActionError("Failed to permanently delete the paper.");
    } finally {
      setDeletingId(null);
    }
  }

  async function handleRestoreProject(id: number) {
    setRestoringProjectId(id);
    setActionError(null);
    try {
      await restoreProject(id);
      await invalidateProjectMutationQueries(qc);
    } catch (e) {
      console.error(e);
      setActionError("Failed to restore the project.");
    } finally {
      setRestoringProjectId(null);
    }
  }

  async function handleDeleteProject(id: number) {
    setDeletingProjectId(id);
    setActionError(null);
    try {
      await hardDeleteProject(id);
      await invalidateProjectMutationQueries(qc);
    } catch (e) {
      console.error(e);
      setActionError("Failed to permanently delete the project.");
    } finally {
      setDeletingProjectId(null);
    }
  }

  return (
    <>
      <KeepInProjectsDialog
        prompt={projectPrompt}
        removing={removing}
        removeError={removeError}
        onKeep={closeProjectPrompt}
        onRemove={handleRemoveFromProjects}
      />
      <SettingGroupLabel>Trash</SettingGroupLabel>
      <p className="mb-2.5 text-xs text-muted">
        Deleted items are kept for 30 days, then permanently removed.
      </p>
      <SettingGroup block>
        {actionError && (
          <p className="text-xs text-danger mb-3">{actionError}</p>
        )}
        {isLoading ? (
          <div className="flex items-center gap-2 py-3 text-sm text-muted">
            <Spinner size={14} /> Loading…
          </div>
        ) : total === 0 ? (
          <p className="text-sm text-muted py-2">Trash is empty</p>
        ) : (
          <>
            {projects.length > 0 && (
              <>
                <p className="text-xs font-semibold text-muted mb-1 mt-2">Projects</p>
                {projects.map((p) => (
                  <ProjectTrashRow
                    key={p.id}
                    project={p}
                    onRestore={() => handleRestoreProject(p.id)}
                    onDelete={() => handleDeleteProject(p.id)}
                    restoring={restoringProjectId === p.id}
                    deleting={deletingProjectId === p.id}
                  />
                ))}
              </>
            )}
            {papers.length > 0 && (
              <>
                <p className="text-xs font-semibold text-muted mb-1 mt-2">Papers</p>
                {papers.map((paper) => (
                  <TrashRow
                    key={paper.source_id}
                    paper={paper}
                    onRestore={() => handleRestore(paper)}
                    onDelete={() => handleDelete(paper.source_id)}
                    restoring={restoringId === paper.source_id}
                    deleting={deletingId === paper.source_id}
                  />
                ))}
              </>
            )}
          </>
        )}
      </SettingGroup>
    </>
  );
}
