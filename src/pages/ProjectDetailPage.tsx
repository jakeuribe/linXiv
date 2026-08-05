import { useEffect, useRef, useState } from "react";
import { useParams, useNavigate, useNavigationType, Link } from "react-router";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, Download, FolderOpen, GitFork, Upload } from "lucide-react";
import { useUiStore, type ExportFormatKey } from "../stores/ui";
import {
  getProject,
  updateProject,
  createProject,
  addPapersToProject,
  removePaperFromProject,
  archiveProject,
  restoreProject,
  deleteProject,
} from "../api/projects";
import { listReceived, sharingAvailable } from "../api/share";
import { receivedShareRole } from "../lib/shareRole";
import { listPapers } from "../api/papers";
import { exportProject, exportBibtex, exportObsidian } from "../api/exportImport";
import { ImportDialog } from "../components/import/ImportDialog";
import type { Paper, Project } from "../types/api";
import { useSelectionStore } from "../stores/selection";
import { ColorSwatch } from "../components/projects/ColorSwatch";
import { PRESET_COLORS } from "../components/projects/constants";
import { TagInput, type TagInputHandle } from "../components/projects/TagInput";
import { Button } from "../components/ui/button";
import { TagBadge } from "../components/tags/TagBadge";
import { Dialog } from "../components/ui/dialog";
import { submitOnCtrlEnter, formSubmitOnCtrlEnter } from "../lib/submitShortcut";
import { Input } from "../components/ui/input";
import { Textarea } from "../components/ui/input";
import { Spinner } from "../components/ui/spinner";
import { EmptyState } from "../components/ui/empty-state";
import { normalizeAuthors } from "../lib/papers";
import { MathText } from "../lib/tex";
import { READING_LIST_TAG, isReadingListProject } from "../lib/readingStatus";
import { useReadingStatusStore } from "../stores/readingStatus";
import { StatusButton } from "../components/reading/StatusButton";

// ---------------------------------------------------------------------------
// Edit Project Dialog
// ---------------------------------------------------------------------------
interface EditProjectDialogProps {
  open: boolean;
  onClose: () => void;
  projectId: number;
  initialName: string;
  initialDescription: string;
  initialColor: string | null;
  initialTags: string[];
}

function EditProjectDialog({
  open,
  onClose,
  projectId,
  initialName,
  initialDescription,
  initialColor,
  initialTags,
}: EditProjectDialogProps) {
  const queryClient = useQueryClient();
  const [name, setName] = useState(initialName);
  const [description, setDescription] = useState(initialDescription);
  const [color, setColor] = useState<string | null>(initialColor);
  const [tags, setTags] = useState<string[]>(initialTags);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const tagInputRef = useRef<TagInputHandle>(null);

  // Keep a mutable ref so the effect reads latest props without dependencies.
  const seedRef = useRef({ initialName, initialDescription, initialColor, initialTags });
  seedRef.current = { initialName, initialDescription, initialColor, initialTags };

  useEffect(() => {
    if (!open) return;
    setName(seedRef.current.initialName);
    setDescription(seedRef.current.initialDescription);
    setColor(seedRef.current.initialColor);
    setTags(seedRef.current.initialTags);
    setError(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (submitting) return;
    if (!name.trim()) return;
    setSubmitting(true);
    setError(null);
    try {
      // Read via imperative handle to capture any uncommitted draft text
      // (typed but not yet Enter'd — stale closure on tags state is unsafe here).
      const currentTags = tagInputRef.current?.getTagsWithDraft() ?? tags;
      const hasReadingListTag = currentTags.some(
        (t) => t.toLowerCase() === READING_LIST_TAG
      );
      const hadReadingListTag = initialTags.some(
        (t) => t.toLowerCase() === READING_LIST_TAG
      );
      if (hasReadingListTag && !hadReadingListTag) {
        setError("Cannot add reading-list tag to a non-reading-list project");
        setSubmitting(false);
        return;
      }
      await updateProject(projectId, {
        name: name.trim(),
        description: description.trim(),
        color_hex: color,
        project_tags: currentTags,
      });
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["projects"] }),
        queryClient.invalidateQueries({ queryKey: ["project", String(projectId)] }),
        queryClient.invalidateQueries({ queryKey: ["tags"] }),
        queryClient.invalidateQueries({ queryKey: ["tag"] }),
      ]);
      onClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to update project");
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Dialog open={open} onClose={onClose} title="Edit Project">
      <form onSubmit={handleSubmit} onKeyDown={formSubmitOnCtrlEnter} className="flex flex-col gap-4">
        <div className="flex flex-col gap-1.5">
          <label
            htmlFor="edit-proj-name"
            className="text-xs font-medium"
            style={{ color: "var(--color-muted)" }}
          >
            Name <span style={{ color: "var(--color-danger)" }}>*</span>
          </label>
          <Input
            id="edit-proj-name"
            value={name}
            onChange={(e) => setName(e.target.value)}
            required
            autoFocus
          />
        </div>

        <div className="flex flex-col gap-1.5">
          <label
            htmlFor="edit-proj-desc"
            className="text-xs font-medium"
            style={{ color: "var(--color-muted)" }}
          >
            Description
          </label>
          <Textarea
            id="edit-proj-desc"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
          />
        </div>

        <div className="flex flex-col gap-1.5">
          <span
            className="text-xs font-medium"
            style={{ color: "var(--color-muted)" }}
          >
            Color
          </span>
          <div className="flex items-center gap-2">
            {PRESET_COLORS.map((c) => (
              <ColorSwatch
                key={c}
                color={c}
                size={20}
                selected={color === c}
                onClick={() => setColor(color === c ? null : c)}
              />
            ))}
          </div>
        </div>

        <div className="flex flex-col gap-1.5">
          <label
            htmlFor="edit-proj-tags"
            className="text-xs font-medium"
            style={{ color: "var(--color-muted)" }}
          >
            Tags
          </label>
          <TagInput ref={tagInputRef} id="edit-proj-tags" value={tags} onChange={setTags} />
          <p className="text-xs" style={{ color: "var(--color-muted)" }}>
            Press Enter to add a tag. Backspace removes the last tag.
          </p>
        </div>

        {error && (
          <p className="text-xs" style={{ color: "var(--color-danger)" }}>
            {error}
          </p>
        )}

        <div className="flex justify-end gap-2 pt-1">
          <Button type="button" variant="muted" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" disabled={!name.trim() || submitting}>
            {submitting ? <Spinner size={14} /> : "Save"}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}

// ---------------------------------------------------------------------------
// Add Papers Dialog
// ---------------------------------------------------------------------------
interface AddPapersDialogProps {
  open: boolean;
  onClose: () => void;
  projectId: number;
  existingSourceIds: string[];
}

function AddPapersDialog({
  open,
  onClose,
  projectId,
  existingSourceIds,
}: AddPapersDialogProps) {
  const queryClient = useQueryClient();
  const [search, setSearch] = useState("");
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const { data: papersData, isLoading } = useQuery({
    queryKey: ["papers"],
    queryFn: () => listPapers(),
    enabled: open,
  });

  // Reset on open
  useEffect(() => {
    if (open) {
      setSearch("");
      setSelectedIds(new Set());
      setError(null);
    }
  }, [open]);

  const candidates = (papersData?.papers ?? []).filter(
    (p) => !existingSourceIds.includes(p.source_id)
  );

  const filtered = candidates.filter((p) =>
    search.trim()
      ? p.title.toLowerCase().includes(search.toLowerCase()) ||
        p.source_id.toLowerCase().includes(search.toLowerCase())
      : true
  );

  function toggleId(id: string) {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  }

  async function handleSubmit() {
    if (submitting) return;
    if (selectedIds.size === 0) return;
    setSubmitting(true);
    setError(null);
    try {
      const { failed } = await addPapersToProject(projectId, [...selectedIds]);
      await queryClient.invalidateQueries({ queryKey: ["project", String(projectId)] });
      // The ["projects"] list query backs the note scope picker / badges.
      await queryClient.invalidateQueries({ queryKey: ["projects"] });
      if (failed.length > 0) {
        setError(`Failed to add ${failed.length} paper${failed.length !== 1 ? "s" : ""}`);
      } else {
        onClose();
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to add papers");
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Dialog open={open} onClose={onClose} title="Add Papers">
      <div className="flex flex-col gap-3">
        <Input
          placeholder="Search papers..."
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          autoFocus
        />

        <div
          className="overflow-y-auto rounded-md border border-[var(--color-border)]"
          style={{ maxHeight: 280, backgroundColor: "var(--color-bg)" }}
          onKeyDown={submitOnCtrlEnter(handleSubmit)}
        >
          {isLoading ? (
            <div className="flex items-center justify-center p-6">
              <Spinner size={20} />
            </div>
          ) : filtered.length === 0 ? (
            <p
              className="p-4 text-sm text-center"
              style={{ color: "var(--color-muted)" }}
            >
              {candidates.length === 0
                ? "All library papers are already in this project"
                : "No papers match your search"}
            </p>
          ) : (
            filtered.map((paper) => (
              <label
                key={paper.source_id}
                className="flex items-start gap-3 px-3 py-2.5 cursor-pointer transition-colors hover:bg-[var(--color-panel)]"
                style={{ borderBottom: "1px solid var(--color-border)" }}
              >
                <input
                  type="checkbox"
                  checked={selectedIds.has(paper.source_id)}
                  onChange={() => toggleId(paper.source_id)}
                  className="mt-0.5 accent-[var(--color-accent)] shrink-0"
                />
                <div className="flex flex-col gap-0.5 min-w-0">
                  <span
                    className="text-sm font-medium leading-snug line-clamp-2"
                    style={{ color: "var(--color-text)" }}
                  >
                    <MathText forceInline>{paper.title}</MathText>
                  </span>
                  <span
                    className="text-xs truncate"
                    style={{ color: "var(--color-muted)" }}
                    title={paper.source_id}
                  >
                    {normalizeAuthors(paper.authors).join(", ") || paper.source_id}
                  </span>
                </div>
              </label>
            ))
          )}
        </div>

        {error && (
          <p className="text-xs" style={{ color: "var(--color-danger)" }}>
            {error}
          </p>
        )}

        <div className="flex items-center justify-between pt-1">
          <span className="text-xs" style={{ color: "var(--color-muted)" }}>
            {selectedIds.size > 0 ? `${selectedIds.size} selected` : ""}
          </span>
          <div className="flex gap-2">
            <Button type="button" variant="muted" onClick={onClose}>
              Cancel
            </Button>
            <Button
              onClick={handleSubmit}
              disabled={selectedIds.size === 0 || submitting}
            >
              {submitting ? <Spinner size={14} /> : "Add"}
            </Button>
          </div>
        </div>
      </div>
    </Dialog>
  );
}

// ---------------------------------------------------------------------------
// Paper row (minimal inline version — full PaperCard lives in Library)
// ---------------------------------------------------------------------------
interface PaperRowProps {
  paper: Paper;
  checked: boolean;
  onToggle: () => void;
  /** Project being viewed; passed as nav state so the note scope picker
   *  on the paper detail page pre-selects it (ADR 0003). */
  projectId: number;
  /** Owning project; used only to detect reading-list projects for the status pill. */
  project: Pick<Project, "project_tags">;
  /** Viewer read-only shares hide the selection checkbox (spec §7). */
  selectable: boolean;
}

function PaperRow({ paper, checked, onToggle, projectId, project, selectable }: PaperRowProps) {
  const navigate = useNavigate();
  const authors = Array.isArray(paper.authors)
    ? paper.authors.slice(0, 3).join(", ")
    : paper.authors;

  return (
    <div
      className="flex items-start gap-3 px-4 py-3 transition-colors hover:bg-[var(--color-panel)]"
      style={{ borderBottom: "1px solid var(--color-border)" }}
    >
      {selectable && (
        <input
          type="checkbox"
          checked={checked}
          onChange={onToggle}
          className="mt-1 accent-[var(--color-accent)] shrink-0 cursor-pointer"
          onClick={(e) => e.stopPropagation()}
        />
      )}
      <div
        className="flex-1 min-w-0 cursor-pointer"
        onClick={() =>
          navigate(`/library/${paper.source_fk}`, {
            state: { fromProjectId: projectId },
          })
        }
      >
        <p
          className="text-sm font-medium leading-snug line-clamp-2"
          style={{ color: "var(--color-text)" }}
        >
          <MathText forceInline>{paper.title}</MathText>
        </p>
        {authors && (
          <p className="text-xs mt-0.5 truncate" style={{ color: "var(--color-muted)" }}>
            {authors}
          </p>
        )}
        <p className="text-xs mt-0.5 truncate" style={{ color: "var(--color-muted)" }}>
          {paper.source_id}
        </p>
      </div>
      {isReadingListProject(project) && (
        <StatusButton sourceId={paper.source_id} />
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Export Dialog
// ---------------------------------------------------------------------------
function ExportDialog({
  open,
  onClose,
  projectId,
  projectName,
}: {
  open: boolean;
  onClose: () => void;
  projectId: number;
  projectName?: string;
}) {
  const exportMethods = useUiStore((s) => s.exportMethods);
  const [includePdfs, setIncludePdfs] = useState(false);
  const [busy, setBusy] = useState<ExportFormatKey | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (open) {
      setError(null);
      setIncludePdfs(false);
    }
  }, [open]);

  async function handleExport(format: ExportFormatKey) {
    setBusy(format);
    setError(null);
    try {
      if (format === "lxproj") {
        await exportProject(projectId, includePdfs, projectName);
      } else if (format === "bibtex") {
        await exportBibtex(projectId, projectName);
      } else if (format === "obsidian") {
        await exportObsidian(projectId, projectName);
      } else {
        format satisfies never;
      }
      onClose();
    } catch (e) {
      if (e instanceof Error && e.name === "AbortError") return; // file-picker cancelled — keep dialog open, show nothing
      setError(e instanceof Error ? e.message : "Export failed");
    } finally {
      setBusy(null);
    }
  }

  const anyEnabled = Object.values(exportMethods).some(Boolean);

  return (
    <Dialog open={open} onClose={onClose} title="Export Project">
      <div className="flex flex-col gap-4">
        {exportMethods.lxproj && (
          <div className="flex flex-col gap-3">
            <label className="flex items-center gap-2 text-sm cursor-pointer select-none" style={{ color: "var(--color-text)" }}>
              <input
                type="checkbox"
                checked={includePdfs}
                onChange={(e) => setIncludePdfs(e.target.checked)}
                className="accent-[var(--color-accent)]"
              />
              Include PDFs in .lxproj archive
            </label>
            {(exportMethods.bibtex || exportMethods.obsidian) && (
              <p className="text-xs" style={{ color: "var(--color-muted)" }}>
                BibTeX and Obsidian exports include paper metadata only.
              </p>
            )}
          </div>
        )}

        {!anyEnabled && (
          <p className="text-sm" style={{ color: "var(--color-muted)" }}>
            No export formats are enabled. Enable them in Settings → Export Methods.
          </p>
        )}

        {error && (
          <p className="text-xs" style={{ color: "var(--color-danger)" }}>{error}</p>
        )}

        <div className="flex gap-2 justify-end pt-1 flex-wrap">
          <Button variant="muted" onClick={onClose}>Cancel</Button>
          {exportMethods.bibtex && (
            <Button variant="muted" onClick={() => handleExport("bibtex")} disabled={!!busy}>
              {busy === "bibtex" ? <Spinner size={14} /> : <><Download size={13} className="mr-1" />BibTeX</>}
            </Button>
          )}
          {exportMethods.obsidian && (
            <Button variant="muted" onClick={() => handleExport("obsidian")} disabled={!!busy}>
              {busy === "obsidian" ? <Spinner size={14} /> : <><Download size={13} className="mr-1" />Obsidian</>}
            </Button>
          )}
          {exportMethods.lxproj && (
            <Button onClick={() => handleExport("lxproj")} disabled={!!busy}>
              {busy === "lxproj" ? <Spinner size={14} /> : <><Download size={13} className="mr-1" />.lxproj</>}
            </Button>
          )}
        </div>
      </div>
    </Dialog>
  );
}

// ---------------------------------------------------------------------------
// Main page
// ---------------------------------------------------------------------------
export default function ProjectDetailPage() {
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();
  const navType = useNavigationType();
  const queryClient = useQueryClient();

  const { selectedIds, toggle, clear, selectAll } = useSelectionStore();

  // Clear selection on mount/unmount.
  useEffect(() => {
    clear();
    return () => clear();
  }, [id, clear]);

  const [editOpen, setEditOpen] = useState(false);
  const [addPapersOpen, setAddPapersOpen] = useState(false);
  const [exportOpen, setExportOpen] = useState(false);
  const [moreOpen, setMoreOpen] = useState(false);
  const moreRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!moreOpen) return;
    function handleOutside(e: MouseEvent) {
      if (moreRef.current && !moreRef.current.contains(e.target as Node)) {
        setMoreOpen(false);
        setConfirmDelete(false);
      }
    }
    function handleEsc(e: KeyboardEvent) {
      if (e.key === "Escape") { setMoreOpen(false); setConfirmDelete(false); }
    }
    document.addEventListener("mousedown", handleOutside);
    document.addEventListener("keydown", handleEsc);
    return () => {
      document.removeEventListener("mousedown", handleOutside);
      document.removeEventListener("keydown", handleEsc);
    };
  }, [moreOpen]);

  const [statusBusy, setStatusBusy] = useState(false);
  const [statusError, setStatusError] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [importOpen, setImportOpen] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [removeError, setRemoveError] = useState<string | null>(null);

  const projectId = id && /^\d+$/.test(id) ? parseInt(id, 10) : NaN;

  const {
    data: project,
    isLoading: projectLoading,
    isError: projectError,
    error: projectFetchError,
  } = useQuery({
    queryKey: ["project", id],
    queryFn: () => getProject(projectId),
    enabled: !isNaN(projectId),
  });

  const {
    data: papersData,
    isLoading: papersLoading,
  } = useQuery({
    queryKey: ["papers"],
    queryFn: () => listPapers(),
    enabled: Boolean(project),
  });

  // §7 viewer read-only: a project linked (share_id) to a received share where
  // our capability is viewer renders with NO edit affordances (hidden, not
  // disabled). Unknown role (offline / plain / hosted) → editable as today;
  // the write boundary itself is enforced host+crypto side, this is UX.
  const { data: receivedShares } = useQuery({
    queryKey: ["share", "received"],
    queryFn: listReceived,
    enabled: sharingAvailable && Boolean(project?.share_id),
  });
  const readOnly = receivedShareRole(project, receivedShares) === "viewer";

  // §7 fork: deep-copy the shared project into an independent local project
  // the user owns (new ids, never synced back), then jump to it.
  async function handleFork() {
    if (!project || statusBusy) return;
    setStatusBusy(true);
    setStatusError(null);
    try {
      const { project: created } = await createProject({
        name: `${project.name} (copy)`,
        description: project.description,
        color_hex: project.color_hex,
        project_tags: project.project_tags,
      });
      // ponytail: copies project metadata + paper links; notes/annotations are
      // library-global in linXiv, so they need no per-project copy.
      await addPapersToProject(created.id, project.source_ids);
      await queryClient.invalidateQueries({ queryKey: ["projects"] });
      navigate(`/projects/${created.id}`);
    } catch (err) {
      setStatusError(err instanceof Error ? err.message : "Failed to fork project");
    } finally {
      setStatusBusy(false);
    }
  }

  const projectPapers: Paper[] = project && papersData
    ? papersData.papers.filter((p) =>
        project.source_ids.includes(p.source_id)
      )
    : [];

  async function handleRemoveSelected() {
    if (selectedIds.size === 0) return;
    setRemoving(true);
    setRemoveError(null);
    try {
      const idsArray = [...selectedIds];
      const results = await Promise.allSettled(
        idsArray.map((sid) => removePaperFromProject(projectId, sid))
      );
      const failedCount = results.filter((r) => r.status === "rejected").length;
      if (failedCount > 0) {
        selectAll(idsArray.filter((_, i) => results[i].status === "rejected"));
        setRemoveError(`Failed to remove ${failedCount} paper${failedCount !== 1 ? "s" : ""}`);
      } else {
        // Clean up reading status entries for successfully removed papers (reading-list projects only).
        if (project && isReadingListProject(project)) {
          idsArray.forEach((sid) => {
            useReadingStatusStore.getState().remove(sid);
          });
        }
        clear();
      }
      await queryClient.invalidateQueries({ queryKey: ["project", id] });
      // Invalidate ["projects"] list for paper detail scope picker.
      await queryClient.invalidateQueries({ queryKey: ["projects"] });
    } catch (err) {
      setRemoveError(err instanceof Error ? err.message : "Failed to remove papers");
    } finally {
      setRemoving(false);
    }
  }

  async function handleArchive() {
    setStatusBusy(true);
    setStatusError(null);
    try {
      await archiveProject(projectId);
      await queryClient.invalidateQueries({ queryKey: ["projects"] });
      navigate("/projects");
    } catch (err) {
      setStatusError(err instanceof Error ? err.message : "Failed to archive project");
    } finally {
      setStatusBusy(false);
    }
  }

  async function handleRestore() {
    setStatusBusy(true);
    setStatusError(null);
    try {
      await restoreProject(projectId);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["projects"] }),
        queryClient.invalidateQueries({ queryKey: ["project", id] }),
      ]);
    } catch (err) {
      setStatusError(err instanceof Error ? err.message : "Failed to restore project");
    } finally {
      setStatusBusy(false);
    }
  }

  async function handleDelete() {
    if (!confirmDelete) {
      setConfirmDelete(true);
      return;
    }
    setConfirmDelete(false);
    setStatusBusy(true);
    setStatusError(null);
    try {
      // Clean up reading status entries if this is a reading-list project.
      if (project && isReadingListProject(project)) {
        projectPapers.forEach((paper) => {
          useReadingStatusStore.getState().remove(paper.source_id);
        });
      }
      await deleteProject(projectId);
      await queryClient.invalidateQueries({ queryKey: ["projects"] });
      navigate("/projects");
    } catch (err) {
      setStatusError(err instanceof Error ? err.message : "Failed to delete project");
    } finally {
      setStatusBusy(false);
    }
  }

  // ------ Render states ------
  if (projectLoading) {
    return (
      <div className="flex-1 flex items-center justify-center h-full">
        <Spinner size={28} />
      </div>
    );
  }

  if (projectError || !project) {
    return (
      <div className="p-8">
        <Link
          to="/projects"
          className="inline-flex items-center gap-1.5 text-sm mb-4 transition-colors"
          style={{ color: "var(--color-muted)" }}
        >
          <ArrowLeft size={14} /> Projects
        </Link>
        <p className="text-sm" style={{ color: "var(--color-danger)" }}>
          {projectFetchError instanceof Error
            ? projectFetchError.message
            : "Project not found"}
        </p>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-6 p-8 overflow-y-auto">
      {/* Back nav */}
      <button
        onClick={() => navType !== "POP" ? navigate(-1) : navigate("/projects")}
        className="inline-flex items-center gap-1.5 text-sm w-fit transition-colors text-muted hover:text-text"
      >
        <ArrowLeft size={14} />
        Back
      </button>

      {/* Header */}
      <div className="flex flex-col gap-3">
        <div className="flex items-start justify-between gap-4">
          <div className="flex items-center gap-3 min-w-0">
            <ColorSwatch color={project.color_hex} size={16} />
            <h1
              className="text-2xl font-semibold leading-tight truncate"
              style={{ color: "var(--color-text)" }}
            >
              {project.name}
            </h1>
            {readOnly && (
              <span
                className="shrink-0 rounded-full border px-2 py-1 font-mono text-[10.5px] font-semibold leading-none"
                style={{
                  color: "var(--color-muted)",
                  borderColor: "var(--color-border)",
                  backgroundColor: "var(--color-surface-2)",
                }}
              >
                Viewer · read only
              </span>
            )}
          </div>
          <div className="flex items-center gap-2 shrink-0 flex-wrap">
            {readOnly && (
              <Button variant="muted" size="sm" onClick={handleFork} disabled={statusBusy}>
                {statusBusy ? (
                  <Spinner size={13} />
                ) : (
                  <>
                    <GitFork size={13} className="mr-1" />Fork to my library
                  </>
                )}
              </Button>
            )}
            {!readOnly && (
              <Button variant="muted" size="sm" onClick={() => setImportOpen(true)}>
                <Upload size={13} className="mr-1" />Import
              </Button>
            )}
            <Button variant="muted" size="sm" onClick={() => setExportOpen(true)}>
              <Download size={13} className="mr-1" />Export
            </Button>
            {!readOnly && (
              <Button variant="muted" size="sm" onClick={() => setEditOpen(true)}>
                Edit
              </Button>
            )}
            {!readOnly && (
            <div className="relative" ref={moreRef}>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => { setMoreOpen((v) => !v); setConfirmDelete(false); }}
                aria-label="More actions"
                aria-haspopup="menu"
                aria-expanded={moreOpen}
              >
                ···
              </Button>
              {moreOpen && (
                <div
                  role="menu"
                  className="absolute right-0 top-full mt-1 z-50 rounded-md border border-border shadow-lg py-1 min-w-28"
                  style={{ background: "var(--color-panel)" }}
                >
                  {project.status === "active" && (
                    <button
                      type="button"
                      role="menuitem"
                      className="w-full text-left px-3 py-1.5 text-sm hover:bg-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-text)] disabled:opacity-40"
                      onClick={() => { setMoreOpen(false); handleArchive(); }}
                      disabled={statusBusy}
                    >
                      {statusBusy ? <Spinner size={12} /> : "Archive"}
                    </button>
                  )}
                  {project.status === "archived" && (
                    <button
                      type="button"
                      role="menuitem"
                      className="w-full text-left px-3 py-1.5 text-sm hover:bg-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-text)] disabled:opacity-40"
                      onClick={() => { setMoreOpen(false); handleRestore(); }}
                      disabled={statusBusy}
                    >
                      {statusBusy ? <Spinner size={12} /> : "Restore"}
                    </button>
                  )}
                  <button
                    type="button"
                    role="menuitem"
                    className="w-full text-left px-3 py-1.5 text-sm hover:bg-[var(--color-border)] disabled:opacity-40"
                    style={{ color: "var(--color-danger)" }}
                    onClick={handleDelete}
                    disabled={statusBusy}
                  >
                    {confirmDelete ? "Confirm delete?" : "Delete"}
                  </button>
                </div>
              )}
            </div>
            )}
          </div>
        </div>

        {project.description && (
          <p className="text-sm" style={{ color: "var(--color-muted)" }}>
            {project.description}
          </p>
        )}

        {statusError && (
          <p className="text-xs" style={{ color: "var(--color-danger)" }}>
            {statusError}
          </p>
        )}

        {project.project_tags.filter((t) => t.toLowerCase() !== READING_LIST_TAG).length > 0 && (
          <div className="flex items-center gap-2 flex-wrap">
            {project.project_tags
              .filter((t) => t.toLowerCase() !== READING_LIST_TAG)
              .map((tag) => (
                <TagBadge key={tag} label={tag} />
              ))}
          </div>
        )}
      </div>

      {/* Papers section */}
      <div className="flex flex-col gap-3">
        <div className="flex items-center justify-between">
          <h2
            className="text-base font-semibold"
            style={{ color: "var(--color-text)" }}
          >
            Papers in this project
          </h2>
          {!readOnly && (
            <Button
              variant="muted"
              size="sm"
              onClick={() => setAddPapersOpen(true)}
            >
              Add Papers
            </Button>
          )}
        </div>

        {/* Selection action bar */}
        {!readOnly && selectedIds.size > 0 && (
          <div
            className="flex items-center justify-between rounded-lg px-4 py-2.5"
            style={{
              backgroundColor: "var(--color-panel)",
              border: "1px solid var(--color-border)",
            }}
          >
            <span className="text-sm" style={{ color: "var(--color-text)" }}>
              {selectedIds.size} paper{selectedIds.size !== 1 ? "s" : ""} selected
            </span>
            <div className="flex items-center gap-2">
              {removeError && (
                <span
                  className="text-xs"
                  style={{ color: "var(--color-danger)" }}
                >
                  {removeError}
                </span>
              )}
              <Button variant="muted" size="sm" onClick={clear}>
                Clear
              </Button>
              <Button
                variant="danger"
                size="sm"
                onClick={handleRemoveSelected}
                disabled={removing}
              >
                {removing ? <Spinner size={12} /> : "Remove from Project"}
              </Button>
            </div>
          </div>
        )}

        {/* Papers list */}
        <div
          className="rounded-lg border border-[var(--color-border)] overflow-hidden"
          style={{ backgroundColor: "var(--color-bg)" }}
        >
          {papersLoading ? (
            <div className="flex items-center justify-center p-8">
              <Spinner size={22} />
            </div>
          ) : projectPapers.length === 0 ? (
            <EmptyState
              icon={<FolderOpen size={28} strokeWidth={1.5} />}
              title="No papers in this project"
              description={
                readOnly
                  ? "This shared project has no papers yet."
                  : "Add papers from your library to start organizing this project."
              }
              actionLabel={readOnly ? undefined : "Add Papers"}
              onAction={readOnly ? undefined : () => setAddPapersOpen(true)}
            />
          ) : (
            projectPapers.map((paper) => (
              <PaperRow
                key={paper.source_id}
                paper={paper}
                checked={selectedIds.has(paper.source_id)}
                onToggle={() => toggle(paper.source_id)}
                projectId={projectId}
                project={project}
                selectable={!readOnly}
              />
            ))
          )}
        </div>
      </div>

      {/* TODO: project-level notes */}
      {/* Notes are available on individual paper detail pages within this project. */}
      {/* A project-level notes panel could be added here once the API supports */}
      {/* querying notes by project_id without requiring a source_id. */}

      {/* Dialogs (edit affordances unmounted entirely on viewer shares) */}
      {project && (
        <>
          {!readOnly && (
            <EditProjectDialog
              key={projectId}
              open={editOpen}
              onClose={() => setEditOpen(false)}
              projectId={projectId}
              initialName={project.name}
              initialDescription={project.description}
              initialColor={project.color_hex}
              initialTags={project.project_tags}
            />
          )}
          {!readOnly && (
            <AddPapersDialog
              key={projectId}
              open={addPapersOpen}
              onClose={() => setAddPapersOpen(false)}
              projectId={projectId}
              existingSourceIds={project.source_ids}
            />
          )}
          <ExportDialog
            key={projectId}
            open={exportOpen}
            onClose={() => setExportOpen(false)}
            projectId={projectId}
            projectName={project.name}
          />
          {!readOnly && (
            <ImportDialog
              open={importOpen}
              onClose={() => setImportOpen(false)}
              projectId={projectId}
              onDone={(newProjectIds) => {
                setImportOpen(false);
                queryClient.invalidateQueries({ queryKey: ["project", id] });
                queryClient.invalidateQueries({ queryKey: ["papers"] });
                const newId = newProjectIds[0];
                if (newId && newId !== projectId) {
                  navigate(`/projects/${newId}`);
                }
              }}
            />
          )}
        </>
      )}
    </div>
  );
}
