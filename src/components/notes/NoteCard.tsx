import { useRef } from "react";
import { useNavigate } from "react-router";
import type { Note, Project } from "../../types/api";
import { Button } from "../ui/button";
import { Badge } from "../ui/badge";
import { formatDate } from "../../lib/date";
import { useConfirmWithTimeout } from "../../hooks/useConfirmWithTimeout";
import { MathText } from "../../lib/tex";
import { NoteBody, noteEdited } from "./NoteBody";

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
  const navigate = useNavigate();
  const cardRef = useRef<HTMLDivElement>(null);
  // Clicking the title/preview opens the full note on its own page so a long
  // note can be read without dropping into the editor. id is effectively always
  // set, but guard so a malformed note isn't a dead click.
  const openNote = note.id != null ? () => navigate(`/notes/${note.id}`) : undefined;
  // A drag-select that ends inside this region still fires click; don't navigate
  // away while the user is selecting preview text to copy. Only a selection
  // inside THIS card suppresses the click — a selection elsewhere shouldn't.
  const handleCardClick = openNote
    ? () => {
        const sel = window.getSelection();
        if (
          sel &&
          !sel.isCollapsed &&
          sel.anchorNode &&
          cardRef.current?.contains(sel.anchorNode)
        ) {
          return;
        }
        openNote();
      }
    : undefined;

  const { date: stamp, edited: isEdited } = noteEdited(note);
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
      {/* Title + preview open the full-note page; actions row below stays separate. */}
      <div
        ref={cardRef}
        role={openNote ? "button" : undefined}
        tabIndex={openNote ? 0 : undefined}
        aria-label={openNote ? `Open note: ${note.title || "Untitled note"}` : undefined}
        onClick={handleCardClick}
        onKeyDown={
          openNote
            ? (e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  openNote();
                }
              }
            : undefined
        }
        className={
          "flex flex-col gap-1.5" +
          (openNote
            ? " cursor-pointer rounded -m-1 p-1 transition-colors hover:bg-[var(--color-border)]"
            : "")
        }
      >
        {/* Header: title + scope badge + date */}
        {/* Title on its own full-width row so it never competes with the scope
            badge for width; a long project-name badge truncates instead of
            starving the title (was: title + shrink-0 badge on one row, which
            crammed the title into a skinny column and clipped the badge when the
            reader divider narrowed). */}
        <div className="flex flex-col gap-1">
          <span className="font-medium text-text leading-snug">
            <MathText forceInline>{note.title || "Untitled note"}</MathText>
          </span>
          <div className="flex items-center gap-2 min-w-0">
            <Badge className="max-w-[240px] min-w-0" color={scopeProject?.color_hex ?? undefined}>
              <span className="truncate">{scopeLabel}</span>
            </Badge>
            <span className="text-muted text-xs shrink-0 whitespace-nowrap">
              {formatDate(stamp)}
              {isEdited && " · edited"}
            </span>
          </div>
        </div>

        {/* Content preview */}
        {note.content && (
          <NoteBody
            content={note.content}
            className="text-muted text-sm line-clamp-3 leading-relaxed"
          />
        )}
      </div>

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
