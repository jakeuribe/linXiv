import { useState, useEffect, useCallback } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router";
import { getSettings, updateSettings } from "../../api/settings";
import { backupDatabase, restoreDatabase } from "../../api/storage";
import { isTauri } from "../../api/client";
import { listSavedPdfs, deleteSavedPdf } from "../../api/pdfs";
import type { SavedPdf } from "../../api/pdfs";
import { getPaperPdfUrl } from "../../api/papers";
import type { PdfPreviewResult } from "../../pages/PdfPreviewPage";
import { useConfirmWithTimeout } from "../../hooks/useConfirmWithTimeout";
import { invalidatePaperMutationQueries } from "../../lib/paperMutations";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { Spinner } from "../ui/spinner";
import { SettingGroup, SettingGroupLabel, SettingRow } from "./SettingRow";
import { errText } from "../../lib/errText";

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

function StatusMessage({ msg }: { msg: { ok: boolean; text: string } | null }) {
  if (!msg) return null;
  return (
    <span
      className={msg.ok ? "text-xs text-success truncate max-w-[220px]" : "text-xs text-danger truncate max-w-[220px]"}
      title={msg.text}
      role="status"
    >
      {msg.text}
    </span>
  );
}

export function StorageSection() {
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
  const [pdfsExpanded, setPdfsExpanded] = useState(false);

  useEffect(() => {
    if (typeof settings?.pdf_save_limit_mb === "number") {
      setPdfLimit(String(settings.pdf_save_limit_mb));
    }
  }, [settings?.pdf_save_limit_mb]);

  const limitNum = Number(pdfLimit);
  const limitValid = pdfLimit !== "" && Number.isInteger(limitNum) && limitNum >= 1;

  const { mutate: save, isPending: saving, isError: saveError } = useMutation({
    mutationFn: () => updateSettings({ pdf_save_limit_mb: limitNum }),
  });

  const [backupMsg, setBackupMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const { mutate: runBackup, isPending: backingUp } = useMutation({
    mutationFn: backupDatabase,
    onSuccess: (info) =>
      setBackupMsg(info ? { ok: true, text: `Saved ${formatBytes(info.bytes)} to ${info.path}` } : null),
    onError: (e) =>
      setBackupMsg({ ok: false, text: errText(e, "Backup failed") }),
  });

  const restoreGuard = useConfirmWithTimeout();
  const [restoreMsg, setRestoreMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const { mutate: runRestore, isPending: restoring } = useMutation({
    mutationFn: restoreDatabase,
    onSuccess: (done) => {
      if (!done) return; // picker cancelled
      setRestoreMsg({ ok: true, text: "Database restored. Restart linXiv to finish." });
      qc.invalidateQueries();
    },
    onError: (e) =>
      setRestoreMsg({ ok: false, text: errText(e, "Restore failed") }),
  });

  const {
    data: pdfData,
    isLoading: pdfsLoading,
    isError: pdfsError,
  } = useQuery({
    queryKey: ["saved-pdfs"],
    queryFn: listSavedPdfs,
    staleTime: 30_000,
    enabled: pdfsExpanded,
  });
  const savedPdfs = pdfData?.pdfs ?? [];

  const invalidateAfterDelete = useCallback(() => {
    invalidatePaperMutationQueries(qc);
  }, [qc]);

  function viewPdf(pdf: SavedPdf) {
    const result: PdfPreviewResult = {
      source_id: pdf.source_id,
      title: pdf.title,
      version: pdf.version,
      paper_url: getPaperPdfUrl(pdf.source_id, pdf.version),
    };
    navigate("/pdf-preview", { state: { result, isSaved: true } });
  }

  return (
    <div>
      <SettingGroupLabel>Storage</SettingGroupLabel>
      {!isTauri && (
        <p className="mb-2.5 text-xs text-muted italic">
          Available in the desktop app. The browser dev build can't backup or restore the database.
        </p>
      )}
      <SettingGroup>
        {/* pdf_save_limit_mb: a total-storage cap enforced by the backend before every new
            PDF write (core's import_pdf + download_pdf sum the managed PDF dir first). */}
        <SettingRow
          label="Total PDF storage (MB)"
          description="Maximum combined disk space for all locally saved PDFs."
        >
          {settingsLoading ? (
            <span className="flex items-center gap-2 text-sm text-muted">
              <Spinner size={14} /> Loading…
            </span>
          ) : settingsError ? (
            <span className="text-xs text-danger">Could not load settings.</span>
          ) : (
            <>
              <Input
                type="number"
                value={pdfLimit}
                onChange={(e) => setPdfLimit(e.target.value)}
                min={1}
                style={{ width: 120 }}
                aria-label="Total PDF storage (MB)"
              />
              <Button size="sm" disabled={!limitValid || saving} onClick={() => save()}>
                {saving ? "Saving…" : "Save"}
              </Button>
              {saveError && (
                <span className="text-xs text-danger">Failed to save.</span>
              )}
            </>
          )}
        </SettingRow>

        <SettingRow
          label="Back up database"
          description="Save a consistent snapshot of your database to a file. Also available as: linxiv backup <dest>"
        >
          <Button
            size="sm"
            disabled={!isTauri || backingUp || restoring}
            onClick={() => {
              setBackupMsg(null);
              runBackup();
            }}
          >
            {backingUp ? "Backing up…" : "Back up…"}
          </Button>
          <StatusMessage msg={backupMsg} />
        </SettingRow>
        <SettingRow
          label="Restore database"
          description="Replace your database with a backup snapshot, then restart linXiv. Also available as: linxiv restore <src>"
        >
          <Button
            size="sm"
            disabled={!isTauri || backingUp || restoring}
            onClick={() => {
              if (restoreGuard.confirm) {
                restoreGuard.disarm();
                setRestoreMsg(null);
                runRestore();
              } else {
                restoreGuard.arm();
              }
            }}
            onMouseDown={(e) => e.preventDefault()}
            onBlur={restoreGuard.disarm}
            className={restoreGuard.confirm ? "text-[var(--color-danger)]" : undefined}
          >
            {restoring ? "Restoring…" : restoreGuard.confirm ? "Replace library?" : "Restore…"}
          </Button>
          <StatusMessage msg={restoreMsg} />
        </SettingRow>
        <p className="mt-2 text-xs text-muted">
          Reading list status (unread/reading/read) and locally saved PDF files are stored locally and not included in backups.
        </p>
      </SettingGroup>

      <div className="mt-8 flex items-center gap-2">
        <SettingGroupLabel className="mb-0">Saved PDFs</SettingGroupLabel>
        <button
          type="button"
          aria-expanded={pdfsExpanded}
          onClick={() => setPdfsExpanded((v) => !v)}
          className="text-muted hover:text-text transition-colors"
          aria-label={pdfsExpanded ? "Collapse saved PDFs list" : "Expand saved PDFs list"}
        >
          <svg
            width="14"
            height="14"
            viewBox="0 0 14 14"
            fill="none"
            aria-hidden="true"
            style={{
              transform: pdfsExpanded ? "rotate(180deg)" : "rotate(0deg)",
              transition: "transform 150ms ease",
            }}
          >
            <path
              d="M2.5 5L7 9.5L11.5 5"
              stroke="currentColor"
              strokeWidth="1.5"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          </svg>
        </button>
      </div>
      {pdfsExpanded && (
        <SettingGroup block className="mt-2.5">
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
        </SettingGroup>
      )}
    </div>
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
    onError: (e) => setErr(errText(e, "Delete failed")),
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
