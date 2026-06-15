import { useState } from "react";
import { Badge } from "../ui/badge";
import { Spinner } from "../ui/spinner";
import type { SearchResult } from "../../types/api";
import { isArxivId } from "../../lib/papers";
import { MathText } from "../../lib/tex";

interface ResultRowProps {
  result: SearchResult;
  saved: boolean;
  onSave: (sourceId: string) => Promise<void>;
  onViewPdf: (result: SearchResult, isSaved: boolean) => void;
}

export function ResultRow({ result, saved, onSave, onViewPdf }: ResultRowProps) {
  const [expanded, setExpanded] = useState(false);
  const [saving, setSaving] = useState(false);
  const [localSaved, setLocalSaved] = useState(false);
  // Derive saved state from both the parent prop (updated by searches/appends) and local optimistic save.
  const isSaved = saved || localSaved;

  const displayAuthors = result.authors.slice(0, 3);
  const moreAuthors = result.authors.length - 3;

  const published = result.published
    ? result.published.slice(0, 10)
    : null;

  async function handleCheck(e: React.ChangeEvent<HTMLInputElement>) {
    if (!e.target.checked || isSaved) return;
    setSaving(true);
    try {
      await onSave(result.source_id);
      setLocalSaved(true);
    } catch (err) {
      // No toast system available; log so devtools surfaces the failure.
      // The checkbox reverts visually (localSaved stays false), which is correct UX.
      console.error("Failed to save paper:", err);
    } finally {
      setSaving(false);
    }
  }

  return (
    <div
      className="border-b border-[var(--color-border)] last:border-b-0"
    >
      <div
        className="flex items-start gap-3 px-4 py-3 hover:bg-[var(--color-panel)] transition-colors cursor-pointer"
        onClick={() => setExpanded((v) => !v)}
      >
        {/* Checkbox — stop propagation so clicking it doesn't expand */}
        <div
          className="flex items-center pt-0.5 shrink-0"
          onClick={(e) => e.stopPropagation()}
        >
          {saving ? (
            <Spinner size={14} />
          ) : (
            <input
              type="checkbox"
              className="w-4 h-4 accent-[var(--color-accent)] cursor-pointer"
              checked={isSaved}
              onChange={handleCheck}
              aria-label="Save paper"
            />
          )}
        </div>

        <div className="flex-1 min-w-0">
          {/* Title */}
          <p className="font-medium text-[var(--color-text)] leading-snug">
            <MathText forceInline>{result.title}</MathText>
          </p>

          {/* Authors */}
          <p className="text-xs text-[var(--color-muted)] mt-0.5 truncate">
            {displayAuthors.join(", ")}
            {moreAuthors > 0 && (
              <span> +{moreAuthors} more</span>
            )}
          </p>

          {/* Meta row */}
          <div className="flex items-center gap-2 mt-1.5 flex-wrap">
            {published && (
              <span className="text-xs text-[var(--color-muted)]">{published}</span>
            )}
            {result.primary_category && (
              <Badge>{result.primary_category}</Badge>
            )}
          </div>
        </div>

        {/* Expand chevron */}
        <span
          className="text-[var(--color-muted)] text-sm shrink-0 mt-0.5 transition-transform"
          style={{ transform: expanded ? "rotate(180deg)" : "rotate(0deg)" }}
          aria-hidden
        >
          ▾
        </span>
      </div>

      {/* Expanded detail panel */}
      {expanded && (
        <div className="px-4 pb-4 ml-7 border-l-2 border-[var(--color-border)]">
          {/* Full authors */}
          {result.authors.length > 3 && (
            <p className="text-xs text-[var(--color-muted)] mb-2">
              <span className="font-medium text-[var(--color-text)]">Authors: </span>
              {result.authors.join(", ")}
            </p>
          )}

          {/* Abstract */}
          <div className="text-sm text-[var(--color-muted)] leading-relaxed whitespace-pre-line">
            <MathText forceInline>{result.summary}</MathText>
          </div>

          {result.paper_url && (
            isArxivId(result.source_id) ? (
              <button
                type="button"
                className="inline-block mt-2 text-xs text-[var(--color-accent)] hover:underline"
                onClick={(e) => {
                  e.stopPropagation();
                  onViewPdf(result, isSaved);
                }}
              >
                PDF →
              </button>
            ) : (
              <a
                href={result.paper_url}
                target="_blank"
                rel="noopener noreferrer"
                className="inline-block mt-2 text-xs text-[var(--color-accent)] hover:underline"
                onClick={(e) => e.stopPropagation()}
              >
                Open →
              </a>
            )
          )}
        </div>
      )}
    </div>
  );
}
