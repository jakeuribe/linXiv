import { useState, useRef, useEffect } from "react";
import type { Note, Project } from "../../types/api";
import { createNote, updateNote } from "../../api/notes";
import { Input, Textarea } from "../ui/input";
import { Button } from "../ui/button";
import { Select } from "../ui/select";
import { Badge } from "../ui/badge";

interface NoteEditorProps {
  sourceId: string;
  /** Projects the paper belongs to; populates the scope picker. */
  projects?: Project[];
  /** True while the projects list is still loading; keeps the picker shown
   *  (disabled) instead of briefly flipping through the empty read-only state. */
  projectsLoading?: boolean;
  /** Pre-selected scope for a new note (e.g. the project navigated from). */
  defaultProjectId?: number | null;
  initialNote?: Note;
  onSave: () => void;
  onCancel: () => void;
}

export function NoteEditor({
  sourceId,
  projects = [],
  projectsLoading = false,
  defaultProjectId = null,
  initialNote,
  onSave,
  onCancel,
}: NoteEditorProps) {
  const isEditing = !!initialNote;
  const [title, setTitle] = useState(initialNote?.title ?? "");
  const [content, setContent] = useState(initialNote?.content ?? "");
  // Scope is chosen at creation time only. The PATCH endpoint updates title and
  // content but never reassigns PROJECT_FK, so when editing we render the
  // existing scope read-only rather than offering a picker that can't take effect.
  const [projectId, setProjectId] = useState<number | null>(
    initialNote ? initialNote.project_id : defaultProjectId,
  );
  // The projects list (and thus defaultProjectId) may resolve AFTER this editor
  // mounts on a cold load. Re-apply the default once it arrives, but never
  // clobber a choice the user has already made.
  const scopeTouched = useRef(false);
  useEffect(() => {
    if (!isEditing && !scopeTouched.current) {
      setProjectId(defaultProjectId);
    }
  }, [defaultProjectId, isEditing]);

  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const scopeProject =
    projectId == null ? null : projects.find((p) => p.id === projectId);
  const scopeLabel =
    projectId == null
      ? "Global"
      : scopeProject?.name ?? "Project-scoped";

  async function handleSave() {
    setSaving(true);
    setError(null);
    try {
      const trimmedTitle = title.trim();
      const trimmedContent = content.trim();
      if (initialNote) {
        await updateNote(initialNote.id, {
          title: trimmedTitle,
          content: trimmedContent,
        });
      } else {
        await createNote({
          source_id: sourceId,
          project_id: projectId,
          title: trimmedTitle,
          content: trimmedContent,
        });
      }
      onSave();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to save note");
    } finally {
      setSaving(false);
    }
  }

  // Read-only scope when editing, or when the paper is confirmed (not still
  // loading) to belong to no projects — global is then the only possible scope.
  // While projects are loading we keep the picker shown (disabled) so the
  // control type doesn't flip from a badge to a select once they arrive.
  const scopeReadOnly = isEditing || (!projectsLoading && projects.length === 0);

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-col items-start gap-1">
        {scopeReadOnly ? (
          <>
            <span id="note-scope-label" className="text-sm font-medium text-muted shrink-0">
              Scope
            </span>
            <Badge
              color={scopeProject?.color_hex ?? undefined}
              aria-labelledby="note-scope-label"
            >
              {scopeLabel}
            </Badge>
          </>
        ) : (
          <>
            <label
              htmlFor="note-scope"
              className="text-sm font-medium text-muted shrink-0"
            >
              Scope
            </label>
            <Select
              id="note-scope"
              size="md"
              value={projectId == null ? "" : String(projectId)}
              onChange={(e) => {
                scopeTouched.current = true;
                setProjectId(
                  e.target.value === "" ? null : Number(e.target.value),
                );
              }}
              disabled={saving || projectsLoading}
              aria-label="Note scope"
            >
              <option value="">Global</option>
              {projects.map((p) => (
                <option key={p.id} value={String(p.id)}>
                  {p.name}
                </option>
              ))}
            </Select>
          </>
        )}
      </div>
      <Input
        placeholder="Note title"
        value={title}
        onChange={(e) => setTitle(e.target.value)}
        disabled={saving}
      />
      <Textarea
        placeholder="Note content…"
        value={content}
        onChange={(e) => setContent(e.target.value)}
        disabled={saving}
        className="min-h-[120px]"
      />
      {error && (
        <p className="text-sm" style={{ color: "var(--color-danger)" }}>
          {error}
        </p>
      )}
      <div className="flex items-center gap-2 justify-end">
        <Button variant="ghost" size="sm" onClick={onCancel} disabled={saving}>
          Cancel
        </Button>
        <Button
          variant="primary"
          size="sm"
          onClick={handleSave}
          disabled={saving || (!title.trim() && !content.trim())}
        >
          {saving ? "Saving…" : "Save"}
        </Button>
      </div>
    </div>
  );
}
