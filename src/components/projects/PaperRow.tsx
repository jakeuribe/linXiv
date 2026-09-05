import { useNavigate } from "react-router";
import type { Paper, Project } from "../../types/api";
import { MathText } from "../../lib/tex";
import { isReadingListProject } from "../../lib/readingStatus";
import { StatusButton } from "../reading/StatusButton";

// Deliberately NOT PaperCard (src/components/papers/PaperCard.tsx): this is a
// compact list row for the project page. It intentionally omits PaperCard's
// meta row, abstract preview, tag row, and card chrome, takes selection as
// props instead of reading the store, and adds project-specific behavior
// (fromProjectId nav state, reading-status pill, viewer-share checkbox gate).
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
  /** Native context menu (Tauri); browser dev falls through to the default. */
  onContextMenu?: React.MouseEventHandler;
}

export function PaperRow({ paper, checked, onToggle, projectId, project, selectable, onContextMenu }: PaperRowProps) {
  const navigate = useNavigate();
  const authors = paper.authors.slice(0, 3).join(", ");

  return (
    <div
      onContextMenu={onContextMenu}
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
