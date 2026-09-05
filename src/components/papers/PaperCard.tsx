import { memo } from "react";
import type { Paper } from "../../types/api";
import { useSelectionStore } from "../../stores/selection";
import { labelForSource } from "../../lib/papers";
import { MathText } from "../../lib/tex";

const MAX_AUTHORS_DISPLAY = 3;
const MAX_TAGS_DISPLAY = 4;

interface PaperCardProps {
  paper: Paper;
  showCheckbox?: boolean;
  onNavigate: (sfk: number) => void;
  /** Native context menu (Tauri); browser dev falls through to the default.
   *  Takes the paper so pages can pass one stable callback (the card is memoized). */
  onContextMenu?: (e: React.MouseEvent, paper: Paper) => void;
}

export const PaperCard = memo(function PaperCard({
  paper,
  showCheckbox = false,
  onNavigate,
  onContextMenu,
}: PaperCardProps) {
  const isSelected = useSelectionStore((s) => s.selectedIds.has(paper.source_id));
  const toggle = useSelectionStore((s) => s.toggle);

  const authors = paper.authors;
  const displayAuthors = authors.slice(0, MAX_AUTHORS_DISPLAY);
  const hasMoreAuthors = authors.length > MAX_AUTHORS_DISPLAY;

  const allTags = paper.tags ?? [];
  const displayTags = allTags.slice(0, MAX_TAGS_DISPLAY);
  const hiddenTagCount = allTags.length - displayTags.length;

  const rawYear = paper.published ? new Date(paper.published).getFullYear() : null;
  const publishedYear = rawYear !== null && Number.isFinite(rawYear) ? rawYear : null;
  const venueYear =
    paper.journal_ref?.trim() || (publishedYear ? String(publishedYear) : "");

  const sourceLabel = labelForSource(paper);
  const hasMetaRow = Boolean(
    paper.category || sourceLabel || venueYear || paper.has_pdf,
  );

  return (
    <div
      onContextMenu={onContextMenu && ((e) => onContextMenu(e, paper))}
      className={[
        "flex items-start gap-4 bg-panel border border-border shadow-card transition-all",
        isSelected ? "ring-1 ring-accent" : "",
      ].join(" ")}
      style={{ borderRadius: "var(--card-radius)", padding: "var(--card-pad)" }}
    >
      {showCheckbox && (
        <div className="shrink-0 flex items-start">
          <input
            type="checkbox"
            checked={isSelected}
            onChange={() => toggle(paper.source_id)}
            className="mt-1 accent-[var(--color-accent)] cursor-pointer"
            aria-label={`Select ${paper.title}`}
          />
        </div>
      )}
      <button
        type="button"
        aria-label={`Open ${paper.title}`}
        onClick={() => onNavigate(paper.source_fk)}
        className="flex-1 text-left hover:brightness-110 cursor-pointer min-w-0"
      >
        {/* Meta row: category · arXiv id · venue/year · status badge */}
        {hasMetaRow && (
        <span className="flex items-center gap-2.5 mb-2">
          {paper.category && (
            <span
              className="font-mono font-medium shrink-0"
              style={{
                fontSize: "10.5px",
                padding: "2px 7px",
                borderRadius: 5,
                background: "color-mix(in srgb, var(--color-accent) 12%, transparent)",
                color: "var(--color-accent)",
              }}
            >
              {paper.category}
            </span>
          )}
          {sourceLabel && (
            <span className="font-mono text-ink3 min-w-0 truncate" style={{ fontSize: 11 }}>
              {sourceLabel}
            </span>
          )}
          {venueYear && (
            <span
              className="font-mono text-ink3 truncate"
              style={{ fontSize: 11, maxWidth: "12rem" }}
            >
              {(paper.category || sourceLabel) && "· "}
              {venueYear}
            </span>
          )}
          <span className="flex-1" />
          {paper.has_pdf && (
            <span
              className="font-mono font-medium shrink-0 inline-flex items-center"
              style={{
                fontSize: "10.5px",
                padding: "2px 8px",
                borderRadius: 20,
                border: "1px solid var(--color-success)",
                color: "var(--color-success)",
                background: "color-mix(in srgb, var(--color-success) 15%, transparent)",
              }}
            >
              PDF
            </span>
          )}
        </span>
        )}

        {/* Title (serif) */}
        <span
          className="block font-display font-semibold text-text line-clamp-2"
          style={{ fontSize: 18, lineHeight: 1.25, letterSpacing: "-0.01em", marginBottom: 5 }}
        >
          <MathText forceInline>{paper.title}</MathText>
        </span>

        {/* Authors */}
        <span className="block text-muted truncate" style={{ fontSize: "12.5px", marginBottom: 9 }}>
          {displayAuthors.join(", ")}
          {hasMoreAuthors && " et al."}
        </span>

        {/* Abstract preview */}
        {paper.summary && (
          <span className="block text-muted line-clamp-2 leading-relaxed" style={{ fontSize: "12.5px" }}>
            <MathText forceInline>{paper.summary}</MathText>
          </span>
        )}

        {/* Tag row */}
        {displayTags.length > 0 && (
          <span className="flex flex-wrap gap-1.5" style={{ marginTop: 11 }}>
            {displayTags.map((tag) => (
              <span
                key={tag}
                className="font-mono text-ink3"
                style={{ fontSize: "10.5px", padding: "2px 8px", border: "1px solid var(--color-border)", borderRadius: 20 }}
              >
                {tag}
              </span>
            ))}
            {hiddenTagCount > 0 && (
              <span
                className="font-mono text-ink3"
                style={{ fontSize: "10.5px", padding: "2px 8px", border: "1px solid var(--color-border)", borderRadius: 20 }}
              >
                +{hiddenTagCount}
              </span>
            )}
          </span>
        )}
      </button>
    </div>
  );
});
