import { useState } from "react";
import { Badge } from "../ui/badge";
import { Spinner } from "../ui/spinner";
import type { SearchResult } from "../../types/api";
import { isArxivId } from "../../lib/papers";
import { errText } from "../../lib/errText";
import { MathText } from "../../lib/tex";
import { showContextMenu } from "../../lib/contextMenu";

interface ResultRowProps {
  result: SearchResult;
  /** Library membership — the parent's saved-lookup query is the only source. */
  saved: boolean;
  onSave: (sourceId: string) => Promise<void>;
  onViewPdf: (result: SearchResult, isSaved: boolean) => void;
}

export function ResultRow({ result, saved, onSave, onViewPdf }: ResultRowProps) {
  const [expanded, setExpanded] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  const displayAuthors = result.authors.slice(0, 3);
  const moreAuthors = result.authors.length - 3;

  const published = result.published
    ? result.published.slice(0, 10)
    : null;

  async function handleSave() {
    setSaving(true);
    setSaveError(null);
    try {
      await onSave(result.source_id);
      // `saved` flips via the parent's query before onSave resolves; nothing to
      // track here.
    } catch (err) {
      setSaveError(errText(err, "Save failed"));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div
      className="border-b border-[var(--color-border)] last:border-b-0"
      onContextMenu={(e) =>
        showContextMenu(e, [
          {
            text: saved ? "In Library" : "Save to Library",
            action: () => void handleSave(),
            enabled: !saved && !saving,
          },
          ...(result.paper_url && isArxivId(result.source_id)
            ? [{ text: "View PDF", action: () => onViewPdf(result, saved) }]
            : []),
        ])
      }
    >
      <div
        className="flex items-start gap-3 px-4 py-3 hover:bg-[var(--color-panel)] transition-colors cursor-pointer"
        onClick={() => setExpanded((v) => !v)}
      >
        {/* Save action / saved indicator — stop propagation so clicking it doesn't expand */}
        <div
          className="flex items-center pt-0.5 shrink-0"
          onClick={(e) => e.stopPropagation()}
        >
          {saved ? (
            <span
              className="w-4 h-4 flex items-center justify-center text-sm font-bold select-none"
              style={{ color: "var(--color-success)" }}
              title="In library"
              aria-label="In library"
              role="img"
            >
              ✓
            </span>
          ) : saving ? (
            <Spinner size={14} />
          ) : (
            <button
              type="button"
              onClick={handleSave}
              className="w-4 h-4 flex items-center justify-center rounded border text-xs font-bold leading-none cursor-pointer transition-opacity hover:opacity-80"
              style={
                saveError
                  ? { borderColor: "var(--color-danger)", color: "var(--color-danger)" }
                  : { borderColor: "var(--color-border)", color: "var(--color-muted)" }
              }
              title={saveError ? `Save failed: ${saveError} — click to retry` : "Save to library"}
              aria-label={saveError ? `Save failed: ${saveError}. Retry save.` : "Save to library"}
            >
              {saveError ? "!" : "+"}
            </button>
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

          {/* Save failure — shown in place, never silently reverted */}
          {saveError && !saved && (
            <p className="text-xs mt-1" style={{ color: "var(--color-danger)" }}>
              Save failed: {saveError}
            </p>
          )}
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
                  onViewPdf(result, saved);
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
