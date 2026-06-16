import { useState, useEffect, useCallback } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import { getSettings, updateSettings } from "../../api/settings";
import { listSavedPdfs, deleteSavedPdf } from "../../api/pdfs";
import type { SavedPdf } from "../../api/pdfs";
import { getPaperPdfUrl } from "../../api/papers";
import { useConfirmWithTimeout } from "../../hooks/useConfirmWithTimeout";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { Spinner } from "../ui/spinner";
import { Section } from "./Section";

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

export function StorageSection({ defaultOpen = true }: { defaultOpen?: boolean } = {}) {
  const qc = useQueryClient();
  const navigate = useNavigate();

  const {
    data: settings,
    isLoading: settingsLoading,
    isError: settingsError,
  } = useQuery({
    queryKey: ["settings"],
    queryFn: getSettings,
  });

  const [pdfLimit, setPdfLimit] = useState<string>("");

  useEffect(() => {
    if (typeof settings?.pdf_save_limit_mb === "number") {
      setPdfLimit(String(settings.pdf_save_limit_mb));
    }
  }, [settings?.pdf_save_limit_mb]);

  const limitNum = Number(pdfLimit);
  const limitValid = pdfLimit !== "" && Number.isInteger(limitNum) && limitNum >= 1;

  const { mutate: save, isPending: saving, isError: saveError } = useMutation({
    mutationFn: () => updateSettings({ pdf_save_limit_mb: limitNum }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["settings"] }),
  });

  const {
    data: pdfData,
    isLoading: pdfsLoading,
    isError: pdfsError,
  } = useQuery({
    queryKey: ["saved-pdfs"],
    queryFn: listSavedPdfs,
    staleTime: 30_000,
  });
  const savedPdfs = pdfData?.pdfs ?? [];

  const invalidateAfterDelete = useCallback(() => {
    qc.invalidateQueries({ queryKey: ["saved-pdfs"] });
    qc.invalidateQueries({ queryKey: ["papers"] });
    qc.invalidateQueries({ queryKey: ["stats"] });
  }, [qc]);

  function viewPdf(pdf: SavedPdf) {
    navigate("/pdf-preview", {
      state: {
        result: {
          source_id: pdf.source_id,
          title: pdf.title,
          version: pdf.version,
          paper_url: getPaperPdfUrl(pdf.source_id, pdf.version),
        },
        isSaved: true,
      },
    });
  }

  return (
    <Section title="Storage" defaultOpen={defaultOpen}>
      <div className="flex flex-col gap-1 mb-2">
        <label className="text-sm text-muted font-medium">PDF Storage Limit (MB)</label>
        {settingsLoading ? (
          <div className="flex items-center gap-2 py-1 text-sm text-muted">
            <Spinner size={14} /> Loading…
          </div>
        ) : settingsError ? (
          <p className="text-xs text-danger">Could not load settings.</p>
        ) : (
          <>
            <div className="flex gap-2 items-center">
              <Input
                type="number"
                value={pdfLimit}
                onChange={(e) => setPdfLimit(e.target.value)}
                min={1}
                style={{ width: 120 }}
              />
              <Button size="sm" disabled={!limitValid || saving} onClick={() => save()}>
                {saving ? "Saving…" : "Save"}
              </Button>
            </div>
            {saveError && (
              <p className="text-xs text-danger mt-1">Failed to save. Please try again.</p>
            )}
          </>
        )}
      </div>

      <div className="flex flex-col gap-1 mt-4">
        <label className="text-sm text-muted font-medium">Saved PDFs</label>
        {pdfsLoading ? (
          <div className="flex items-center gap-2 py-1 text-sm text-muted">
            <Spinner size={14} /> Loading…
          </div>
        ) : pdfsError ? (
          <p className="text-xs text-danger">Could not load saved PDFs.</p>
        ) : savedPdfs.length === 0 ? (
          <p className="text-xs text-muted">No PDFs saved locally.</p>
        ) : (
          <ul className="flex flex-col divide-y" style={{ borderColor: "var(--color-border)" }}>
            {savedPdfs.map((pdf) => (
              <SavedPdfRow
                key={pdf.source_id}
                pdf={pdf}
                onView={() => viewPdf(pdf)}
                onDeleted={invalidateAfterDelete}
              />
            ))}
          </ul>
        )}
      </div>
    </Section>
  );
}

function SavedPdfRow({
  pdf,
  onView,
  onDeleted,
}: {
  pdf: SavedPdf;
  onView: () => void;
  onDeleted: () => void;
}) {
  const { confirm, arm, disarm } = useConfirmWithTimeout();
  const [err, setErr] = useState<string | null>(null);
  const { mutate: onDelete, isPending: deleting } = useMutation({
    mutationFn: () => deleteSavedPdf(pdf.source_id),
    onSuccess: () => {
      setErr(null);
      onDeleted();
    },
    onError: (e) => setErr(e instanceof Error ? e.message : "Delete failed"),
  });
  return (
    <li className="flex flex-col gap-1 py-2">
      <div className="flex items-center gap-2">
        <button
          type="button"
          onClick={onView}
          title={pdf.title}
          className="flex-1 min-w-0 text-left text-sm text-text hover:text-accent truncate"
        >
          {pdf.title}
        </button>
        <span className="shrink-0 text-xs text-muted tabular-nums">
          {formatBytes(pdf.size_bytes)}
        </span>
        <Button
          variant="ghost"
          size="sm"
          disabled={deleting}
          onClick={() => {
            if (confirm) {
              disarm();
              setErr(null);
              onDelete();
            } else {
              arm();
            }
          }}
          onMouseDown={(e) => e.preventDefault()}
          onBlur={disarm}
          className={
            confirm ? "text-[var(--color-danger)]" : "hover:text-[var(--color-danger)]"
          }
        >
          {deleting ? "Deleting…" : confirm ? "Confirm?" : "Delete"}
        </Button>
      </div>
      {err && <p className="text-xs text-danger">{err}</p>}
    </li>
  );
}
