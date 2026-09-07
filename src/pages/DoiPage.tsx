import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Button } from "../components/ui/button";
import { formSubmitOnCtrlEnter } from "../lib/submitShortcut";
import { invalidatePaperMutationQueries } from "../lib/paperMutations";
import { Input } from "../components/ui/input";
import { Spinner } from "../components/ui/spinner";
import { resolveDoi, saveDoi } from "../api/search";
import type { PaperMetadata } from "../types/api";

export default function DoiPage() {
  const queryClient = useQueryClient();
  const [doi, setDoi] = useState("");
  // Capture the exact DOI string that was resolved, so Save always uses it
  // even if the user edits the input field afterwards.
  const [resolvedDoi, setResolvedDoi] = useState("");
  const [metadata, setMetadata] = useState<PaperMetadata | null>(null);
  const [saveSuccess, setSaveSuccess] = useState(false);

  // Resolve mutation
  const resolveMutation = useMutation({
    mutationFn: (d: string) => resolveDoi(d),
    onSuccess: (data, variables) => {
      setMetadata(data.metadata);
      setResolvedDoi(variables);
      setSaveSuccess(false);
    },
  });

  // Save mutation
  const saveMutation = useMutation({
    mutationFn: (d: string) => saveDoi(d),
    onSuccess: () => {
      setSaveSuccess(true);
      invalidatePaperMutationQueries(queryClient);
    },
  });

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = doi.trim();
    if (!trimmed) return;
    setMetadata(null);
    setSaveSuccess(false);
    resolveMutation.mutate(trimmed);
  }

  function handleClear() {
    setMetadata(null);
    setSaveSuccess(false);
    setDoi("");
    setResolvedDoi("");
    resolveMutation.reset();
    saveMutation.reset();
  }

  const resolveError = resolveMutation.error as Error | null;
  const saveError = saveMutation.error as Error | null;

  return (
    <div className="flex flex-col h-full overflow-y-auto">
      <div className="mx-auto w-full max-w-[640px] px-6 py-8">
        <h1 className="font-display text-[27px] font-semibold leading-tight tracking-[-0.015em] text-text mb-6">
          Add Paper by DOI
        </h1>

        {/* Input form */}
        <form onSubmit={handleSubmit} onKeyDown={formSubmitOnCtrlEnter} className="flex gap-2 mb-2">
          <Input
            placeholder="10.48550/arXiv.2312.00752"
            value={doi}
            onChange={(e) => setDoi(e.target.value)}
            disabled={resolveMutation.isPending}
            className="flex-1 h-9"
            aria-label="DOI"
          />
          <Button
            type="submit"
            variant="primary"
            size="md"
            disabled={resolveMutation.isPending || !doi.trim()}
          >
            {resolveMutation.isPending && <Spinner size={14} />}
            {resolveMutation.isPending ? "Looking up…" : "Look up"}
          </Button>
        </form>

        {/* Resolve error */}
        {resolveError && (
          <p className="text-sm mt-2 mb-4" style={{ color: "var(--color-danger)" }}>
            {resolveError.message}
          </p>
        )}

        {/* Loading state */}
        {resolveMutation.isPending && (
          <div className="flex items-center justify-center gap-3 py-16 text-[var(--color-muted)]">
            <Spinner size={24} />
          </div>
        )}

        {/* Result card */}
        {!resolveMutation.isPending && metadata && (
          <div
            className="rounded-lg border border-[var(--color-border)] p-5 mt-4"
            style={{ background: "var(--color-panel)" }}
          >
            {/* Title */}
            {metadata.title && (
              <h2 className="font-semibold text-[var(--color-text)] leading-snug mb-2">
                {metadata.title}
              </h2>
            )}

            {/* Authors */}
            {metadata.authors.length > 0 && (
              <p className="text-sm text-[var(--color-muted)] mb-3">
                {metadata.authors.join(", ")}
              </p>
            )}

            {/* Abstract (the wire field is `summary`) */}
            {metadata.summary && (
              <p
                className="text-sm text-[var(--color-muted)] leading-relaxed mb-4"
                style={{
                  display: "-webkit-box",
                  WebkitLineClamp: 3,
                  WebkitBoxOrient: "vertical",
                  overflow: "hidden",
                }}
              >
                {metadata.summary}
              </p>
            )}

            {/* Meta row */}
            <div className="flex flex-wrap items-center gap-3 text-xs text-[var(--color-muted)] mb-4 border-t border-[var(--color-border)] pt-3">
              {metadata.source && (
                <span>
                  <span className="text-[var(--color-text)]">Source:</span>{" "}
                  {metadata.source}
                </span>
              )}
              {metadata.doi && (
                <span>
                  <span className="text-[var(--color-text)]">DOI:</span>{" "}
                  {metadata.doi}
                </span>
              )}
              {metadata.published && (
                <span>
                  <span className="text-[var(--color-text)]">Published:</span>{" "}
                  {metadata.published.slice(0, 10)}
                </span>
              )}
            </div>

            {/* Actions */}
            <div className="flex items-center gap-2 flex-wrap">
              {saveSuccess ? (
                <p
                  className="text-sm font-medium"
                  style={{ color: "var(--color-success)" }}
                >
                  Saved to library ✓
                </p>
              ) : (
                <Button
                  type="button"
                  variant="primary"
                  size="sm"
                  disabled={saveMutation.isPending}
                  onClick={() => saveMutation.mutate(resolvedDoi)}
                >
                  {saveMutation.isPending && <Spinner size={12} />}
                  {saveMutation.isPending ? "Saving…" : "Save to Library"}
                </Button>
              )}

              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={handleClear}
              >
                Clear
              </Button>
            </div>

            {/* Save error */}
            {saveError && (
              <p
                className="text-sm mt-2"
                style={{ color: "var(--color-danger)" }}
              >
                {saveError.message}
              </p>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
