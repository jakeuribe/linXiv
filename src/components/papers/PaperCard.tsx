import { memo } from "react";
import type { Paper } from "../../types/api";
import { Badge } from "../ui/badge";
import { useSelectionStore } from "../../stores/selection";
import { normalizeAuthors } from "../../lib/papers";
import { MathText } from "../../lib/tex";

const MAX_AUTHORS_DISPLAY = 3;
const MAX_TAGS_DISPLAY = 4;

interface PaperCardProps {
  paper: Paper;
  showCheckbox?: boolean;
  onNavigate: (sfk: number) => void;
}

export const PaperCard = memo(function PaperCard({
  paper,
  showCheckbox = false,
  onNavigate,
}: PaperCardProps) {
  const isSelected = useSelectionStore((s) => s.selectedIds.has(paper.source_id));
  const toggle = useSelectionStore((s) => s.toggle);

  const authors = normalizeAuthors(paper.authors ?? []);
  const displayAuthors = authors.slice(0, MAX_AUTHORS_DISPLAY);
  const hasMoreAuthors = authors.length > MAX_AUTHORS_DISPLAY;

  const allTags = paper.tags ?? [];
  const displayTags = allTags.slice(0, MAX_TAGS_DISPLAY);
  const hiddenTagCount = allTags.length - displayTags.length;

  const rawYear = paper.published ? new Date(paper.published).getFullYear() : null;
  const publishedYear = rawYear !== null && Number.isFinite(rawYear) ? rawYear : null;

  return (
    // Checkbox and navigation button are siblings — no interactive descendant ARIA violation.
    // All children of <button> are <span> or <p> (phrasing content) — no block elements inside.
    <div
      className={[
        "flex bg-panel rounded-lg border border-border transition-all",
        isSelected ? "ring-1 ring-accent" : "",
      ].join(" ")}
    >
      {showCheckbox && (
        <div className="shrink-0 pt-4 pl-4 flex items-start">
          <input
            type="checkbox"
            checked={isSelected}
            onChange={() => toggle(paper.source_id)}
            className="mt-0.5 accent-[var(--color-accent)] cursor-pointer"
            aria-label={`Select ${paper.title}`}
          />
        </div>
      )}
      <button
        type="button"
        aria-label={`Open ${paper.title}`}
        onClick={() => onNavigate(paper.source_fk)}
        className={[
          "flex-1 text-left py-4 pr-4 hover:brightness-110 cursor-pointer min-w-0",
          showCheckbox ? "pl-2" : "pl-4",
        ].join(" ")}
      >
        {/* Row 1: title + pdf badge */}
        <span className="flex items-start gap-2">
          <span className="flex-1 font-medium text-text leading-snug line-clamp-2">
            <MathText forceInline>{paper.title}</MathText>
          </span>
          {paper.has_pdf && (
            <Badge
              className="shrink-0 ml-1"
              style={{
                borderColor: "var(--color-success)",
                color: "var(--color-success)",
                backgroundColor: "color-mix(in srgb, var(--color-success) 15%, transparent)",
              }}
            >
              PDF
            </Badge>
          )}
        </span>

        {/* Row 2: authors + date */}
        <span className="flex items-center gap-2 mt-1.5 text-muted text-sm">
          <span className="truncate">
            {displayAuthors.join(", ")}
            {hasMoreAuthors && " et al."}
          </span>
          {publishedYear && (
            <>
              <span className="text-border">·</span>
              <span className="shrink-0">{publishedYear}</span>
            </>
          )}
        </span>

        {/* Row 3: category + tags */}
        {(paper.category || displayTags.length > 0) && (
          <span className="flex flex-wrap gap-1.5 mt-2">
            {paper.category && (
              <Badge
                style={{
                  borderColor: "var(--color-accent)",
                  color: "var(--color-accent)",
                  backgroundColor: "color-mix(in srgb, var(--color-accent) 12%, transparent)",
                }}
              >
                {paper.category}
              </Badge>
            )}
            {displayTags.map((tag) => (
              <Badge key={tag}>{tag}</Badge>
            ))}
            {hiddenTagCount > 0 && (
              <Badge>+{hiddenTagCount}</Badge>
            )}
          </span>
        )}

        {/* Abstract preview */}
        {paper.summary && (
          <span className="block mt-2 text-muted text-sm line-clamp-2 leading-relaxed">
            <MathText forceInline>{paper.summary}</MathText>
          </span>
        )}
      </button>
    </div>
  );
});
