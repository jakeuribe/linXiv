import { useEffect, useRef, useState } from "react";
import { useMutation, useMutationState, useQueryClient } from "@tanstack/react-query";
import { Lock, Settings2 } from "lucide-react";
import {
  downloadSharedPdf,
  importReceived,
  listReceivedPapers,
  syncShare,
  type SharedSummary,
  shareErrText,
} from "../../api/share";
import { ApiError } from "../../api/client";
import {
  invalidatePaperMutationQueries,
  invalidateProjectMutationQueries,
} from "../../lib/paperMutations";
import { SHARE_SYNC_MUTATION_KEY } from "../../lib/syncPill";
import { Button } from "../ui/button";
import { Spinner } from "../ui/spinner";

export type ShareRole = "Hoster" | "Reader";

function RolePill({ role }: { role: ShareRole }) {
  const hosted = role === "Hoster";
  return (
    <span
      className="shrink-0 rounded-full border px-2 py-1 font-mono text-[10.5px] font-semibold leading-none"
      style={{
        color: hosted ? "var(--color-accent)" : "var(--color-muted)",
        borderColor: hosted ? "var(--color-accent)" : "var(--color-border)",
        backgroundColor: hosted
          ? "color-mix(in srgb, var(--color-accent) 10%, transparent)"
          : "var(--color-surface-2)",
      }}
    >
      {role}
    </span>
  );
}

function Stat({ value, label }: { value: number; label: string }) {
  return (
    <div>
      <span className="font-display text-[17px] font-semibold text-text">{value}</span>
      <span className="ml-1.5 text-[11.5px]" style={{ color: "var(--color-ink-3)" }}>
        {label}
        {value === 1 ? "" : "s"}
      </span>
    </div>
  );
}

/** "just now" / "5m ago" / "3d ago" from an ISO timestamp. */
export function relAgo(iso: string): string {
  const mins = Math.floor((Date.now() - new Date(iso).getTime()) / 60_000);
  if (!Number.isFinite(mins)) return "recently";
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  if (mins < 24 * 60) return `${Math.floor(mins / 60)}h ago`;
  return `${Math.floor(mins / (24 * 60))}d ago`;
}

/** "Synced 5m ago" from the summary's ISO synced_at. */
function syncedText(iso: string | null): string {
  return iso ? `Synced ${relAgo(iso)}` : "Never synced";
}

const SYNC_REASON_LABELS: Record<string, string | undefined> = {
  "no ticket": "No valid ticket",
  "p2p offline": "P2P offline",
  "project gone": "Project deleted",
  "bad ticket": "Bad ticket",
  "paused": "Sync paused",
  "direction": "Skipped by sync direction",
  "revoked or awaiting key": "Access revoked or key not yet received",
  "awaiting first sync": "The host has not answered yet — nothing to show",
  "no key for any content":
    "Content arrived but none of it decrypts — it was published before your invite, so the host must republish it",
};

function humanizeReason(code: string | undefined): string {
  if (!code) return "Sync failed";
  return SYNC_REASON_LABELS[code] ?? code;
}

/** The reader leg's raw counters, for pasting into a bug report. */
function syncCounters(d: {
  applied?: number;
  no_key?: number;
  failed?: number;
}): string | null {
  if (d.applied == null) return null;
  return `applied ${d.applied} · no key ${d.no_key ?? 0} · failed ${d.failed ?? 0}`;
}

/** Import a received mirror into the library; shared by the card's visible
 * "Import to library" button and the settings dialog's Local-project row.
 * `isPending` is derived across ALL instances of the keyed mutation, so the
 * dialog's button is disabled while the card's import is in flight (and vice
 * versa) instead of firing a duplicate import. */
export function useImportReceived(shareId: string) {
  const queryClient = useQueryClient();
  const key = ["share", "import-received", shareId];
  const m = useMutation({
    mutationKey: key,
    mutationFn: () => importReceived(shareId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["share", "published"] });
      queryClient.invalidateQueries({ queryKey: ["share", "received"] });
      invalidateProjectMutationQueries(queryClient);
    },
  });
  const pendingAnywhere =
    useMutationState({
      filters: { mutationKey: key, status: "pending" },
    }).length > 0;
  return { ...m, isPending: m.isPending || pendingAnywhere };
}

export function ShareCard({
  share,
  role,
  onSettings,
}: {
  share: SharedSummary;
  role: ShareRole;
  onSettings: () => void;
}) {
  const hosted = role === "Hoster";
  const queryClient = useQueryClient();
  const importM = useImportReceived(share.share_id);
  const sync = useMutation({
    // Registers under the shared key so the header SyncStatusPill sees it.
    mutationKey: SHARE_SYNC_MUTATION_KEY,
    mutationFn: () => syncShare(share.share_id),
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ["share", "published"] });
      queryClient.invalidateQueries({ queryKey: ["share", "received"] });
    },
  });
  const resetRef = useRef(sync.reset);
  resetRef.current = sync.reset;
  useEffect(() => {
    resetRef.current();
  }, [share.synced_at, share.paused]);
  // A fresh mirror (unlink → re-import gives a new project_fk) must not show
  // the previous mirror's "Saved N of M PDFs" result.
  const pdfsResetRef = useRef<() => void>(() => {});
  useEffect(() => {
    pdfsResetRef.current();
  }, [share.project_fk]);
  // Sequential fetch of every has_pdf paper; result summarized below.
  const [pdfProgress, setPdfProgress] = useState({ done: 0, total: 0 });
  const pdfCancelRef = useRef(false);
  useEffect(() => {
    return () => {
      pdfCancelRef.current = true;
    };
  }, []);
  const pdfs = useMutation({
    mutationFn: async () => {
      pdfCancelRef.current = false;
      setPdfProgress({ done: 0, total: 0 });
      const papers = (await listReceivedPapers(share.share_id)).filter((p) => p.has_pdf);
      setPdfProgress({ done: 0, total: papers.length });
      let saved = 0;
      let consecutiveFailures = 0;
      let stopped: string | null = null;
      const failed: string[] = [];
      for (const p of papers) {
        if (pdfCancelRef.current) {
          stopped = "cancelled";
          break;
        }
        try {
          await downloadSharedPdf(share.share_id, p.source_id);
          saved++;
          consecutiveFailures = 0;
        } catch (e) {
          failed.push(`${p.title || p.source_id}: ${shareErrText(e)}`);
          if (e instanceof ApiError && e.status === 413) {
            stopped = "storage limit reached";
            break;
          }
          if (++consecutiveFailures >= 3) {
            stopped = `stopped after ${consecutiveFailures} consecutive failures`;
            break;
          }
        }
        setPdfProgress((prog) => ({ ...prog, done: prog.done + 1 }));
      }
      return { total: papers.length, saved, failed, stopped };
    },
    onSuccess: () => {
      invalidatePaperMutationQueries(queryClient);
    },
  });
  pdfsResetRef.current = pdfs.reset;
  return (
    <div className="flex flex-col overflow-hidden rounded-lg border border-[var(--color-border)] bg-[var(--color-panel)]">
      <div className="px-5 pb-3.5 pt-4">
        <div className="flex items-center gap-2.5">
          <span
            className="h-[11px] w-[11px] shrink-0 rounded-[3px]"
            style={{
              backgroundColor: hosted ? "var(--color-accent)" : "var(--color-ink-3)",
            }}
          />
          <span className="font-display flex-1 truncate text-[17px] font-semibold text-text">
            {share.name || "(pending first sync)"}
          </span>
          {share.e2ee && (
            <Lock
              size={13}
              aria-label="End-to-end encrypted"
              style={{ color: "var(--color-muted)" }}
            />
          )}
          {share.pending && (
            <span
              className="shrink-0 rounded-full border border-[var(--color-border)] bg-[var(--color-surface-2)] px-2 py-1 font-mono text-[10.5px] font-semibold leading-none"
              style={{ color: "var(--color-muted)" }}
            >
              Pending
            </span>
          )}
          <RolePill role={role} />
        </div>
      </div>
      <div className="mx-5 flex items-center gap-2 border-y border-[var(--color-border)] py-2.5">
        <span
          className="h-1.5 w-1.5 shrink-0 rounded-full"
          style={{
            backgroundColor: share.paused ? "var(--color-ink-3)" : "var(--color-accent)",
          }}
        />
        <span className="truncate text-xs" style={{ color: "var(--color-muted)" }}>
          {share.paused
            ? "Sync paused"
            : share.pending
              ? "Waiting for the host — nothing has arrived yet"
              : syncedText(share.synced_at)}
          {" · "}
          {hosted ? "published from your library" : "read-only mirror"}
        </span>
      </div>
      <div className="flex items-center gap-4 px-5 pb-4 pt-3">
        <Stat value={share.paper_count} label="paper" />
        <Stat value={share.note_count} label="note" />
        <Stat value={share.tag_count} label="tag" />
        <div className="flex-1" />
        {!hosted && share.e2ee && share.project_fk != null && (
          <>
            <Button
              variant="muted"
              size="sm"
              onClick={() => pdfs.mutate()}
              disabled={pdfs.isPending}
            >
              {pdfs.isPending ? (
                <>
                  <Spinner size={14} /> {pdfProgress.done}/{pdfProgress.total}
                </>
              ) : (
                "Download PDFs"
              )}
            </Button>
            {pdfs.isPending && (
              <Button
                variant="ghost"
                size="sm"
                onClick={() => {
                  pdfCancelRef.current = true;
                }}
              >
                Cancel
              </Button>
            )}
          </>
        )}
        {!hosted && !share.pending && share.project_fk == null && (
          <Button
            variant="primary"
            size="sm"
            onClick={() => importM.mutate()}
            disabled={importM.isPending}
          >
            {importM.isPending ? <Spinner size={14} /> : "Import to library"}
          </Button>
        )}
        <Button
          variant="muted"
          size="sm"
          onClick={() => sync.mutate()}
          disabled={sync.isPending || share.paused}
        >
          {sync.isPending ? <Spinner size={14} /> : "Sync now"}
        </Button>
        <Button variant="ghost" size="sm" aria-label="Share settings" onClick={onSettings}>
          <Settings2 size={15} />
        </Button>
      </div>
      {(sync.isError || sync.data?.synced === false || sync.data?.reason != null) && (
        <p
          className="px-5 pb-3 text-xs"
          style={{
            // "still waiting on the host" is a state, not a failure.
            color: sync.data?.pending
              ? "var(--color-muted)"
              : "var(--color-danger)",
          }}
        >
          {sync.isError ? shareErrText(sync.error) : humanizeReason(sync.data?.reason)}
        </p>
      )}
      {sync.data && syncCounters(sync.data) && (
        <p
          className="px-5 pb-3 font-mono text-[10.5px]"
          style={{ color: "var(--color-ink-3)" }}
        >
          {syncCounters(sync.data)}
        </p>
      )}
      {importM.isError && (
        <p className="px-5 pb-3 text-xs" style={{ color: "var(--color-danger)" }}>
          {shareErrText(importM.error)}
        </p>
      )}
      {pdfs.isError && (
        <p className="px-5 pb-3 text-xs" style={{ color: "var(--color-danger)" }}>
          {shareErrText(pdfs.error)}
        </p>
      )}
      {pdfs.data && (
        <p
          className="px-5 pb-3 text-xs"
          style={{
            color: pdfs.data.failed.length
              ? "var(--color-danger)"
              : "var(--color-muted)",
          }}
        >
          {pdfs.data.total === 0
            ? "No PDFs shared in this project"
            : `Saved ${pdfs.data.saved} of ${pdfs.data.total} PDF${
                pdfs.data.total === 1 ? "" : "s"
              }${pdfs.data.stopped ? ` (${pdfs.data.stopped})` : ""}`}
          {pdfs.data.failed.length > 0 &&
            ` — ${pdfs.data.failed.slice(0, 3).join("; ")}${
              pdfs.data.failed.length > 3
                ? `; and ${pdfs.data.failed.length - 3} more failed`
                : ""
            }`}
        </p>
      )}
    </div>
  );
}
