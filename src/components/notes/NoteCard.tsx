import type { Note, Project } from "../../types/api";
import { Button } from "../ui/button";
import { Badge } from "../ui/badge";
import { formatDate } from "../../lib/date";
import { useConfirmWithTimeout } from "../../hooks/useConfirmWithTimeout";
import { MathText } from "../../lib/tex";

interface NoteCardProps {
  note: Note;
  /** Projects the paper belongs to; used to resolve the scope badge label. */
  projects?: Project[];
  onEdit: (note: Note) => void;
  onDelete: (note: Note) => void;
}

export function NoteCard({ note, projects = [], onEdit, onDelete }: NoteCardProps) {
  // Deleting a note is an irreversible hard-delete (no trash/restore), so gate
  // it behind the same arm-to-confirm step used for the app's other destructive
  // actions rather than firing on a single click.
  const { confirm: confirmDelete, arm, disarm } = useConfirmWithTimeout();

  // created_at and updated_at are equal at creation and diverge on PATCH.
  // Compare parsed instants rather than raw strings so timestamp-formatting
  // differences can't produce a false "edited" flag.
  const createdMs = note.created_at ? Date.parse(note.created_at) : NaN;
  const updatedMs = note.updated_at ? Date.parse(note.updated_at) : NaN;
  const isEdited =
    Number.isFinite(createdMs) &&
    Number.isFinite(updatedMs) &&
    updatedMs !== createdMs;
  const scopeProject =
    note.project_id == null
      ? null
      : projects.find((p) => p.id === note.project_id);
  // A note can be scoped to a project the paper no longer belongs to (or an
  // archived one not in `projects`); fall back to a neutral label rather than
  // implying it is global.
  const scopeLabel = note.project_id == null
    ? "Global"
    : scopeProject?.name ?? "Project-scoped";

  return (
    <div className="bg-panel rounded border border-border p-3 flex flex-col gap-1.5">
      {/* Header: title + scope badge + date */}
      <div className="flex items-start justify-between gap-2">
        <span className="font-medium text-text leading-snug">
          <MathText forceInline>{note.title || "Untitled note"}</MathText>
        </span>
        <div className="shrink-0 flex items-center gap-2 mt-0.5">
          <Badge color={scopeProject?.color_hex ?? undefined}>{scopeLabel}</Badge>
          <span className="text-muted text-xs">
            {formatDate(isEdited ? note.updated_at : note.created_at)}
            {isEdited && " · edited"}
          </span>
        </div>
      </div>

      {/* Content preview */}
      {note.content && (
        <div className="text-muted text-sm line-clamp-3 leading-relaxed whitespace-pre-wrap">
          {note.content.split("\n").map((line, i) => (
            <span key={i + "-" + line}>
              {i > 0 && <br />}
              <MathText forceInline>{line}</MathText>
            </span>
          ))}
        </div>
      )}

      {/* Actions */}
      <div className="flex items-center gap-1 pt-1">
        <Button
          variant="ghost"
          size="sm"
          onClick={() => onEdit(note)}
        >
          Edit
        </Button>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => {
            if (confirmDelete) {
              disarm();
              onDelete(note);
            } else {
              arm();
            }
          }}
          onBlur={disarm}
          className={
            confirmDelete
              ? "text-[var(--color-danger)]"
              : "hover:text-[var(--color-danger)]"
          }
        >
          {confirmDelete ? "Confirm?" : "Delete"}
        </Button>
      </div>
    </div>
  );
}
