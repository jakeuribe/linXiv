import { useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { listProjects, createProject } from "../api/projects";
import { ProjectCard } from "../components/projects/ProjectCard";
import { ColorSwatch } from "../components/projects/ColorSwatch";
import { PRESET_COLORS } from "../components/projects/constants";
import { TagInput, type TagInputHandle } from "../components/projects/TagInput";
import { Button } from "../components/ui/button";
import { Dialog } from "../components/ui/dialog";
import { Input } from "../components/ui/input";
import { Textarea } from "../components/ui/input";
import { Spinner } from "../components/ui/spinner";

function NewProjectDialog({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const queryClient = useQueryClient();
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [color, setColor] = useState<string | null>(null);
  const [tags, setTags] = useState<string[]>([]);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const tagInputRef = useRef<TagInputHandle>(null);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!name.trim()) return;
    setSubmitting(true);
    setError(null);
    try {
      // Read via imperative handle to capture any uncommitted draft text.
      const currentTags = tagInputRef.current?.getTagsWithDraft() ?? tags;
      await createProject({
        name: name.trim(),
        description: description.trim() || undefined,
        color_hex: color,
        project_tags: currentTags.length > 0 ? currentTags : undefined,
      });
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["projects"] }),
        queryClient.invalidateQueries({ queryKey: ["tags"] }),
        queryClient.invalidateQueries({ queryKey: ["tag"] }),
      ]);
      handleClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to create project");
    } finally {
      setSubmitting(false);
    }
  }

  function handleClose() {
    setName("");
    setDescription("");
    setColor(null);
    setTags([]);
    setError(null);
    onClose();
  }

  return (
    <Dialog open={open} onClose={handleClose} title="New Project">
      <form onSubmit={handleSubmit} className="flex flex-col gap-4">
        <div className="flex flex-col gap-1.5">
          <label
            htmlFor="proj-name"
            className="text-xs font-medium"
            style={{ color: "var(--color-muted)" }}
          >
            Name <span style={{ color: "var(--color-danger)" }}>*</span>
          </label>
          <Input
            id="proj-name"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="Project name"
            required
            autoFocus
          />
        </div>

        <div className="flex flex-col gap-1.5">
          <label
            htmlFor="proj-desc"
            className="text-xs font-medium"
            style={{ color: "var(--color-muted)" }}
          >
            Description
          </label>
          <Textarea
            id="proj-desc"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder="Optional description"
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
            htmlFor="new-proj-tags"
            className="text-xs font-medium"
            style={{ color: "var(--color-muted)" }}
          >
            Tags
          </label>
          <TagInput ref={tagInputRef} id="new-proj-tags" value={tags} onChange={setTags} />
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
          <Button type="button" variant="muted" onClick={handleClose}>
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

export default function ProjectsPage() {
  const navigate = useNavigate();
  const [dialogOpen, setDialogOpen] = useState(false);
  const [showArchived, setShowArchived] = useState(false);

  const { data, isLoading, isError, error } = useQuery({
    queryKey: ["projects", showArchived ? "archived" : "active"],
    queryFn: () => listProjects(showArchived ? "archived" : "active"),
  });

  const { data: archivedData } = useQuery({
    queryKey: ["projects", "archived"],
    queryFn: () => listProjects("archived"),
    enabled: !showArchived,
  });

  const projects = data?.projects ?? [];
  const archivedCount = showArchived ? projects.length : (archivedData?.projects?.length ?? 0);

  return (
    <div className="flex flex-col gap-6 p-8 h-full overflow-y-auto">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <h1 className="font-display text-[27px] font-semibold leading-tight tracking-[-0.015em] text-text">
            {showArchived ? "Archived Projects" : "Projects"}
          </h1>
          {!showArchived && archivedCount > 0 && (
            <button
              type="button"
              onClick={() => setShowArchived(true)}
              className="text-xs transition-colors"
              style={{ color: "var(--color-muted)" }}
            >
              {archivedCount} archived
            </button>
          )}
          {showArchived && (
            <button
              type="button"
              onClick={() => setShowArchived(false)}
              className="text-xs transition-colors"
              style={{ color: "var(--color-muted)" }}
            >
              ← back
            </button>
          )}
        </div>
        {!showArchived && (
          <Button onClick={() => setDialogOpen(true)}>New Project</Button>
        )}
      </div>

      {/* Loading */}
      {isLoading && (
        <div className="flex-1 flex items-center justify-center">
          <Spinner size={28} />
        </div>
      )}

      {/* Error */}
      {isError && (
        <div
          className="rounded-lg border p-4 text-sm"
          style={{
            borderColor: "var(--color-danger)",
            color: "var(--color-danger)",
            backgroundColor: "var(--color-panel)",
          }}
        >
          Failed to load projects:{" "}
          {error instanceof Error ? error.message : "Unknown error"}
        </div>
      )}

      {/* Empty state */}
      {!isLoading && !isError && projects.length === 0 && (
        <div
          className="flex-1 flex items-center justify-center text-sm"
          style={{ color: "var(--color-muted)" }}
        >
          {showArchived ? "No archived projects" : "No active projects"}
        </div>
      )}

      {/* Grid */}
      {projects.length > 0 && (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
          {projects.map((project) => (
            <ProjectCard
              key={project.id}
              project={project}
              onClick={() => navigate(`/projects/${project.id}`)}
            />
          ))}
        </div>
      )}

      <NewProjectDialog
        open={dialogOpen}
        onClose={() => setDialogOpen(false)}
      />
    </div>
  );
}
