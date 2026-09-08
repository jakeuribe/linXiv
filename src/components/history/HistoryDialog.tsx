import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { History, RotateCcw } from "lucide-react";
import {
  getChangeDiff,
  getTimeline,
  restoreTo,
  type HistoryScope,
} from "../../api/history";
import type { ChangeRow, EntryChange, HistoryDiff } from "../../types/api";
import { Dialog } from "../ui/dialog";
import { Button } from "../ui/button";
import { Spinner } from "../ui/spinner";
import { EmptyState } from "../ui/empty-state";
import { diffSummary, formatActor, formatTime } from "../../lib/history";
import { errText } from "../../lib/errText";
import { useConfirmWithTimeout } from "../../hooks/useConfirmWithTimeout";

/** Git-log-style history of a project or the whole library: change list on the
 *  left, the selected change's diff on the right, restore per change. */
export function HistoryDialog({
  open,
  onClose,
  scope,
  title,
}: {
  open: boolean;
  onClose: () => void;
  scope: HistoryScope;
  title: string;
}) {
  const queryClient = useQueryClient();
  const [selected, setSelected] = useState<string | null>(null);
  const [restoring, setRestoring] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { confirm, arm, disarm } = useConfirmWithTimeout();

  const scopeKey =
    scope.kind === "project"
      ? `project-${scope.id}`
      : scope.kind === "share"
        ? `share-${scope.shareId}`
        : "library";

  const {
    data: timeline,
    isLoading,
    error: timelineError,
  } = useQuery({
    queryKey: ["history", scopeKey],
    queryFn: () => getTimeline(scope),
    enabled: open,
  });

  // Newest first, like git log.
  const changes = [...(timeline?.changes ?? [])].reverse();
  const latestHash = changes[0]?.hash ?? null;
  const active = selected ?? latestHash;

  const {
    data: diff,
    isLoading: diffLoading,
    error: diffError,
  } = useQuery({
    queryKey: ["history-diff", scopeKey, active],
    queryFn: () => getChangeDiff(scope, active!),
    enabled: open && active !== null,
  });

  useEffect(() => {
    if (open) {
      setSelected(null);
      setError(null);
      disarm();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  async function handleRestore() {
    if (scope.kind === "share" || !active) return;
    if (!confirm) {
      arm();
      return;
    }
    disarm();
    setRestoring(true);
    setError(null);
    try {
      await restoreTo(scope, active);
      // A restore can touch papers, notes, annotations, tags and the project
      // row; refetching everything is the honest invalidation.
      await queryClient.invalidateQueries();
      onClose();
    } catch (e) {
      setError(errText(e, "Restore failed"));
    } finally {
      setRestoring(false);
    }
  }

  return (
    <Dialog open={open} onClose={onClose} title={title} size="2xl">
      {isLoading ? (
        <div className="flex items-center justify-center py-8">
          <Spinner size={18} />
        </div>
      ) : timelineError ? (
        <p className="py-6 text-sm" style={{ color: "var(--color-danger)" }}>
          {errText(timelineError, "Failed to load history")}
        </p>
      ) : changes.length === 0 ? (
        <EmptyState
          icon={<History size={28} />}
          title="No history yet"
          description="Changes are journaled in the background after each edit."
        />
      ) : (
        <div className="flex gap-4 min-h-0" style={{ height: "60vh" }}>
          {/* Change list */}
          <div
            className="flex w-2/5 flex-col gap-1 overflow-y-auto pr-1"
            style={{ borderRight: "1px solid var(--color-border)" }}
          >
            {changes.map((c) => (
              <ChangeItem
                key={c.hash}
                change={c}
                selected={c.hash === active}
                latest={c.hash === latestHash}
                onSelect={() => {
                  setSelected(c.hash);
                  disarm();
                }}
              />
            ))}
          </div>
          {/* Diff panel */}
          <div className="flex min-w-0 flex-1 flex-col gap-2 overflow-y-auto">
            {diffError ? (
              <p className="py-6 text-sm" style={{ color: "var(--color-danger)" }}>
                {errText(diffError, "Failed to load this change's diff")}
              </p>
            ) : diffLoading || !diff ? (
              <div className="flex items-center justify-center py-8">
                <Spinner size={16} />
              </div>
            ) : (
              <>
                <DiffView diff={diff} />
                {scope.kind !== "share" && (
                  <div className="mt-auto flex items-center justify-end gap-2 pt-2">
                    {error && (
                      <span
                        className="text-xs"
                        style={{ color: "var(--color-danger)" }}
                      >
                        {error}
                      </span>
                    )}
                    <Button
                      variant={confirm ? "danger" : "muted"}
                      size="sm"
                      onClick={handleRestore}
                      onBlur={disarm}
                      disabled={restoring || active === latestHash}
                    >
                      {restoring ? (
                        <Spinner size={13} />
                      ) : (
                        <>
                          <RotateCcw size={13} className="mr-1" />
                          {confirm ? "Confirm restore?" : "Restore to here"}
                        </>
                      )}
                    </Button>
                  </div>
                )}
              </>
            )}
          </div>
        </div>
      )}
    </Dialog>
  );
}

function ChangeItem({
  change,
  selected,
  latest,
  onSelect,
}: {
  change: ChangeRow;
  selected: boolean;
  latest: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onSelect}
      className="flex flex-col gap-0.5 rounded-md px-2.5 py-1.5 text-left transition-colors"
      style={{
        backgroundColor: selected ? "var(--color-surface-2)" : "transparent",
        border: `1px solid ${selected ? "var(--color-border)" : "transparent"}`,
      }}
    >
      <span className="flex items-center gap-2 text-xs">
        <span className="font-mono" style={{ color: "var(--color-accent)" }}>
          {change.hash.slice(0, 7)}
        </span>
        <span style={{ color: "var(--color-text)" }}>
          {formatActor(change.actor, change.mine)}
        </span>
        {latest && (
          <span
            className="rounded-full border px-1.5 text-[10px] leading-4"
            style={{
              color: "var(--color-muted)",
              borderColor: "var(--color-border)",
            }}
          >
            latest
          </span>
        )}
      </span>
      <span className="text-[11px]" style={{ color: "var(--color-muted)" }}>
        {formatTime(change.time)}
        {change.message ? ` · ${change.message}` : ""}
      </span>
    </button>
  );
}

function DiffView({ diff }: { diff: HistoryDiff }) {
  const summary = diffSummary(diff);
  if (!summary) {
    return (
      <p className="text-sm" style={{ color: "var(--color-muted)" }}>
        No visible changes (background bookkeeping).
      </p>
    );
  }
  return (
    <div className="flex flex-col gap-1.5 text-sm">
      <p className="text-xs" style={{ color: "var(--color-muted)" }}>
        {summary}
      </p>
      {diff.meta.map((m) => (
        <DiffLine key={`meta-${m.field}`} sign="~">
          {m.field}: {m.from || "∅"} → {m.to || "∅"}
        </DiffLine>
      ))}
      {diff.papers_added.map((p) => (
        <DiffLine key={`pa-${p.source_id}`} sign="+">
          paper · {p.title || p.source_id}
        </DiffLine>
      ))}
      {diff.papers_removed.map((p) => (
        <DiffLine key={`pr-${p.source_id}`} sign="−">
          paper · {p.title || p.source_id}
        </DiffLine>
      ))}
      {diff.tags_added.map((t) => (
        <DiffLine key={`ta-${t}`} sign="+">
          tag · {t}
        </DiffLine>
      ))}
      {diff.tags_removed.map((t) => (
        <DiffLine key={`tr-${t}`} sign="−">
          tag · {t}
        </DiffLine>
      ))}
      {diff.notes_added.map((n) => (
        <EntryLine key={`na-${n.uuid}`} sign="+" noun="note" e={n} />
      ))}
      {diff.notes_changed.map((n) => (
        <EntryLine key={`nc-${n.uuid}`} sign="~" noun="note" e={n} />
      ))}
      {diff.notes_removed.map((n) => (
        <EntryLine key={`nr-${n.uuid}`} sign="−" noun="note" e={n} />
      ))}
      {diff.annotations_added.map((a) => (
        <EntryLine key={`aa-${a.uuid}`} sign="+" noun="annotation" e={a} />
      ))}
      {diff.annotations_changed.map((a) => (
        <EntryLine key={`ac-${a.uuid}`} sign="~" noun="annotation" e={a} />
      ))}
      {diff.annotations_removed.map((a) => (
        <EntryLine key={`ar-${a.uuid}`} sign="−" noun="annotation" e={a} />
      ))}
    </div>
  );
}

const SIGN_COLOR: Record<string, string> = {
  "+": "var(--color-success)",
  "−": "var(--color-danger)",
  "~": "var(--color-accent)",
};

function DiffLine({
  sign,
  children,
}: {
  sign: "+" | "−" | "~";
  children: React.ReactNode;
}) {
  return (
    <div
      className="flex items-baseline gap-2 rounded px-2 py-0.5 font-mono text-xs"
      style={{
        color: SIGN_COLOR[sign],
        backgroundColor: `color-mix(in srgb, ${SIGN_COLOR[sign]} 8%, transparent)`,
      }}
    >
      <span className="select-none">{sign}</span>
      <span className="min-w-0 break-words" style={{ color: "var(--color-text)" }}>
        {children}
      </span>
    </div>
  );
}

function EntryLine({
  sign,
  noun,
  e,
}: {
  sign: "+" | "−" | "~";
  noun: string;
  e: EntryChange;
}) {
  return (
    <div className="flex flex-col">
      <DiffLine sign={sign}>
        {noun} · {e.title || e.uuid.slice(0, 8)}
      </DiffLine>
      {sign === "~" && (
        <div
          className="ml-6 flex flex-col gap-0.5 py-0.5 font-mono text-[11px]"
          style={{ color: "var(--color-muted)" }}
        >
          <span className="break-words" style={{ color: "var(--color-danger)" }}>
            − {e.from}
          </span>
          <span className="break-words" style={{ color: "var(--color-success)" }}>
            + {e.to}
          </span>
        </div>
      )}
    </div>
  );
}
