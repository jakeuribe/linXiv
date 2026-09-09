import { memo, useEffect, useState } from "react";
import { useNavigate } from "react-router";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { History, Pencil, RotateCcw } from "lucide-react";
import {
  getChangeDiff,
  getDeviceActor,
  getTimeline,
  restoreTo,
  type HistoryScope,
} from "../../api/history";
import { remoteMemberCode } from "../../api/remote";
import { getSettings, updateSettings } from "../../api/settings";
import type { ChangeRow, EntryChange, HistoryDiff } from "../../types/api";
import { Dialog } from "../ui/dialog";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { Spinner } from "../ui/spinner";
import { EmptyState } from "../ui/empty-state";
import {
  diffSummary,
  formatActor,
  formatTime,
  isMineChange,
  viewerIdentities,
  wordDiff,
} from "../../lib/history";
import { errText } from "../../lib/errText";

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
  const navigate = useNavigate();
  const [selected, setSelected] = useState<string | null>(null);
  const [restoring, setRestoring] = useState(false);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [actorFilter, setActorFilter] = useState<string | null>(null);
  const [renameActor, setRenameActor] = useState<string | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [renaming, setRenaming] = useState(false);

  // No onClose(): open-state lives in the URL (useUrlDialog), so navigating
  // away closes the dialog — and Back returns to it, still open.
  function go(path: string) {
    navigate(path);
  }

  // Local actor→name overrides (this device's settings). Layered over a
  // remote node's host-assigned display_name.
  const { data: settings } = useQuery({
    queryKey: ["settings"],
    queryFn: getSettings,
    enabled: open,
  });
  const actorNames = settings?.actor_names ?? {};
  const nameFor = (c: ChangeRow) =>
    actorNames[c.actor.toLowerCase()] ?? c.display_name;

  // "mine" from the VIEWER's identities, fetched locally — the wire flag is
  // computed by whichever node served the timeline, which marks that node's
  // own changes as "mine" when browsing a remote library. Two identities:
  // the journal actor (local edits) and the p2p endpoint id (remote-query
  // writes journal under it on the node).
  const { data: deviceActor } = useQuery({
    queryKey: ["device-actor"],
    queryFn: getDeviceActor,
    enabled: open,
    staleTime: Infinity,
  });
  const { data: memberCode } = useQuery({
    queryKey: ["member-code"],
    queryFn: remoteMemberCode,
    enabled: open,
    staleTime: Infinity,
    retry: false, // absent whenever the p2p transport isn't up; hex fallback
  });
  const viewerIds = viewerIdentities(deviceActor?.actor, memberCode);
  const isMine = (c: ChangeRow) => isMineChange(c.actor, c.mine, viewerIds);

  async function saveRename() {
    if (!renameActor) return;
    const key = renameActor.toLowerCase();
    const name = renameValue.trim();
    setRenaming(true);
    setError(null);
    try {
      // Fresh read, not the query cache: a failed/stale ['settings'] query
      // would base the map on {} and wipe every other saved name.
      const current = (await getSettings()).actor_names ?? {};
      const next = { ...current };
      if (name) next[key] = name;
      else delete next[key];
      await updateSettings({ actor_names: next });
      setRenameActor(null);
    } catch (e) {
      setError(errText(e, "Saving the name failed"));
    } finally {
      setRenaming(false);
    }
  }

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

  // Newest first, like git log. latest/restore semantics come from the
  // UNFILTERED list — an actor filter only narrows what's shown.
  const changes = [...(timeline?.changes ?? [])].reverse();
  const latestHash = changes[0]?.hash ?? null;
  const active = selected ?? latestHash;
  const visible = actorFilter
    ? changes.filter((c) => c.actor === actorFilter)
    : changes;
  const filterChange = actorFilter
    ? changes.find((c) => c.actor === actorFilter)
    : undefined;

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
      setConfirmOpen(false);
      setActorFilter(null);
      setRenameActor(null);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  async function handleRestore() {
    if (scope.kind === "share" || !active) return;
    setRestoring(true);
    setError(null);
    try {
      await restoreTo(scope, active);
      // A restore can touch papers, notes, annotations, tags and the project
      // row; refetching everything is the honest invalidation.
      await queryClient.invalidateQueries();
      setConfirmOpen(false);
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
            {actorFilter && (
              <button
                type="button"
                onClick={() => setActorFilter(null)}
                className="self-start rounded-full border px-1.5 text-[10px] leading-4"
                style={{
                  color: "var(--color-muted)",
                  borderColor: "var(--color-border)",
                }}
              >
                Showing:{" "}
                {formatActor(
                  actorFilter,
                  filterChange ? isMine(filterChange) : false,
                  filterChange ? nameFor(filterChange) : undefined
                )}{" "}
                ×
              </button>
            )}
            {visible.map((c) => (
              <ChangeItem
                key={c.hash}
                change={c}
                selected={c.hash === active}
                latest={c.hash === latestHash}
                onSelect={() => {
                  setSelected(c.hash);
                  setError(null);
                }}
                onActorClick={() =>
                  setActorFilter(actorFilter === c.actor ? null : c.actor)
                }
                displayName={nameFor(c)}
                mine={isMine(c)}
                onRename={() => {
                  setRenameValue(actorNames[c.actor.toLowerCase()] ?? "");
                  setError(null);
                  setRenameActor(c.actor);
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
                {scope.kind !== "share" && (
                  <div className="flex items-center justify-end pb-1">
                    <Button
                      variant="muted"
                      size="sm"
                      onClick={() => {
                        setError(null);
                        setConfirmOpen(true);
                      }}
                      disabled={restoring || active === latestHash}
                    >
                      <RotateCcw size={13} className="mr-1" />
                      Restore to here
                    </Button>
                  </div>
                )}
                <DiffView diff={diff} go={go} />
              </>
            )}
          </div>
        </div>
      )}
      <Dialog
        open={confirmOpen}
        onClose={() => {
          if (!restoring) setConfirmOpen(false);
        }}
        title="Restore to this change?"
      >
        <div className="flex flex-col gap-4">
          <p className="text-sm" style={{ color: "var(--color-muted)" }}>
            This rolls everything in scope back to{" "}
            <span className="font-mono" style={{ color: "var(--color-accent)" }}>
              {active?.slice(0, 7)}
            </span>
            . The restore itself is journaled, so it can be undone from history.
          </p>
          {error && (
            <p className="text-xs" style={{ color: "var(--color-danger)" }}>
              {error}
            </p>
          )}
          <div className="flex items-center justify-end gap-2">
            <Button
              variant="muted"
              size="sm"
              onClick={() => setConfirmOpen(false)}
              disabled={restoring}
            >
              Cancel
            </Button>
            <Button
              variant="danger"
              size="sm"
              onClick={handleRestore}
              disabled={restoring}
            >
              {restoring ? (
                <Spinner size={13} />
              ) : (
                <>
                  <RotateCcw size={13} className="mr-1" />
                  Restore
                </>
              )}
            </Button>
          </div>
        </div>
      </Dialog>
      <Dialog
        open={renameActor !== null}
        onClose={() => {
          if (!renaming) setRenameActor(null);
        }}
        title="Name this person"
      >
        <div className="flex flex-col gap-4">
          <p className="text-sm" style={{ color: "var(--color-muted)" }}>
            Shown instead of{" "}
            <span className="font-mono" style={{ color: "var(--color-accent)" }}>
              {renameActor?.slice(0, 8)}
            </span>{" "}
            on this device only. Leave empty to clear.
          </p>
          <Input
            value={renameValue}
            onChange={(e) => setRenameValue(e.target.value)}
            placeholder="name"
            maxLength={64}
            autoFocus
            onKeyDown={(e) => {
              if (e.key === "Enter") void saveRename();
            }}
          />
          {error && (
            <p className="text-xs" style={{ color: "var(--color-danger)" }}>
              {error}
            </p>
          )}
          <div className="flex items-center justify-end gap-2">
            <Button
              variant="muted"
              size="sm"
              onClick={() => setRenameActor(null)}
              disabled={renaming}
            >
              Cancel
            </Button>
            <Button size="sm" onClick={saveRename} disabled={renaming}>
              {renaming ? <Spinner size={13} /> : "Save"}
            </Button>
          </div>
        </div>
      </Dialog>
    </Dialog>
  );
}

function ChangeItem({
  change,
  selected,
  latest,
  onSelect,
  onActorClick,
  displayName,
  mine,
  onRename,
}: {
  change: ChangeRow;
  selected: boolean;
  latest: boolean;
  onSelect: () => void;
  onActorClick: () => void;
  displayName: string | null | undefined;
  mine: boolean;
  onRename: () => void;
}) {
  return (
    // A plain div (row-wide click is a mouse convenience): the hash button
    // is the keyboard-selectable control, and the byline/pencil are sibling
    // <button>s — no focusable descendants inside an ARIA button.
    <div
      onClick={onSelect}
      className="flex cursor-pointer flex-col gap-0.5 rounded-md px-2.5 py-1.5 text-left transition-colors"
      style={{
        backgroundColor: selected ? "var(--color-surface-2)" : "transparent",
        border: `1px solid ${selected ? "var(--color-border)" : "transparent"}`,
      }}
    >
      <span className="flex items-center gap-2 text-xs">
        <button
          type="button"
          aria-label={`Select change ${change.hash.slice(0, 7)}`}
          className="font-mono"
          style={{ color: "var(--color-accent)" }}
          onClick={(e) => {
            e.stopPropagation();
            onSelect();
          }}
        >
          {change.hash.slice(0, 7)}
        </button>
        {/* title = full actor hex, hover-copyable for the operator's pairing
            workflow; click toggles filtering the list to this actor. */}
        <button
          type="button"
          style={{ color: "var(--color-text)" }}
          title={change.actor}
          onClick={(e) => {
            e.stopPropagation();
            onActorClick();
          }}
        >
          {formatActor(change.actor, mine, displayName)}
        </button>
        {/* Naming your own device is invisible ("This device" wins). */}
        {!mine && (
          <button
            type="button"
            aria-label="Name this person"
            title="Name this person"
            className="opacity-40 hover:opacity-100"
            style={{ color: "var(--color-muted)" }}
            onClick={(e) => {
              e.stopPropagation();
              onRename();
            }}
          >
            <Pencil size={11} />
          </button>
        )}
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
    </div>
  );
}

function DiffView({
  diff,
  go,
}: {
  diff: HistoryDiff;
  go: (path: string) => void;
}) {
  const summary = diffSummary(diff);
  if (!summary) {
    return (
      <p className="text-sm" style={{ color: "var(--color-muted)" }}>
        No visible changes (background bookkeeping).
      </p>
    );
  }
  const noteTo = (n: EntryChange) =>
    n.note_id != null ? `/notes/${n.note_id}` : null;
  const annTo = (a: EntryChange) =>
    a.paper_sfk != null ? `/library/${a.paper_sfk}` : null;
  return (
    <div className="flex flex-col gap-1.5 text-sm">
      <p className="text-xs" style={{ color: "var(--color-muted)" }}>
        {summary}
      </p>
      {diff.meta.map((m) => (
        <DiffLine key={`meta-${m.field}`} sign="~">
          {/* An empty side means cleared/newly set — keep the explicit ∅
              (a word diff of an empty string renders nothing there). */}
          {m.field}:{" "}
          {m.from && m.to ? (
            <InlineWordDiff from={m.from} to={m.to} />
          ) : (
            <>
              {m.from || "∅"} → {m.to || "∅"}
            </>
          )}
        </DiffLine>
      ))}
      {diff.papers_added.map((p) => (
        <DiffLine key={`pa-${p.source_id}`} sign="+">
          paper ·{" "}
          <EntityLink
            to={p.source_fk != null ? `/library/${p.source_fk}` : null}
            go={go}
          >
            {p.title || p.source_id}
          </EntityLink>
        </DiffLine>
      ))}
      {diff.papers_removed.map((p) => (
        <DiffLine key={`pr-${p.source_id}`} sign="−">
          paper ·{" "}
          <EntityLink
            to={p.source_fk != null ? `/library/${p.source_fk}` : null}
            go={go}
          >
            {p.title || p.source_id}
          </EntityLink>
        </DiffLine>
      ))}
      {diff.tags_added.map((t) => (
        <DiffLine key={`ta-${t}`} sign="+">
          tag ·{" "}
          <EntityLink to={`/tags/${encodeURIComponent(t)}`} go={go}>
            {t}
          </EntityLink>
        </DiffLine>
      ))}
      {diff.tags_removed.map((t) => (
        <DiffLine key={`tr-${t}`} sign="−">
          tag ·{" "}
          <EntityLink to={`/tags/${encodeURIComponent(t)}`} go={go}>
            {t}
          </EntityLink>
        </DiffLine>
      ))}
      {diff.notes_added.map((n) => (
        <EntryLine key={`na-${n.uuid}`} sign="+" noun="note" e={n} to={noteTo(n)} go={go} />
      ))}
      {diff.notes_changed.map((n) => (
        <EntryLine key={`nc-${n.uuid}`} sign="~" noun="note" e={n} to={noteTo(n)} go={go} />
      ))}
      {diff.notes_removed.map((n) => (
        <EntryLine key={`nr-${n.uuid}`} sign="−" noun="note" e={n} to={noteTo(n)} go={go} />
      ))}
      {diff.annotations_added.map((a) => (
        <EntryLine key={`aa-${a.uuid}`} sign="+" noun="annotation" e={a} to={annTo(a)} go={go} />
      ))}
      {diff.annotations_changed.map((a) => (
        <EntryLine key={`ac-${a.uuid}`} sign="~" noun="annotation" e={a} to={annTo(a)} go={go} />
      ))}
      {diff.annotations_removed.map((a) => (
        <EntryLine key={`ar-${a.uuid}`} sign="−" noun="annotation" e={a} to={annTo(a)} go={go} />
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
  to,
  go,
}: {
  sign: "+" | "−" | "~";
  noun: string;
  e: EntryChange;
  to: string | null;
  go: (path: string) => void;
}) {
  return (
    <div className="flex flex-col">
      <DiffLine sign={sign}>
        {noun} ·{" "}
        <EntityLink to={to} go={go}>
          {e.title || e.uuid.slice(0, 8)}
        </EntityLink>
      </DiffLine>
      {/* changed rows always carry both sides (diff_projects). */}
      {sign === "~" && (
        <div className="ml-6 break-words py-0.5 font-mono text-[11px]">
          <InlineWordDiff from={e.from ?? ""} to={e.to ?? ""} />
        </div>
      )}
    </div>
  );
}

/** Entity text as a subtle in-app link when a local target exists; plain
 *  text otherwise. */
function EntityLink({
  to,
  go,
  children,
}: {
  to: string | null;
  go: (path: string) => void;
  children: React.ReactNode;
}) {
  if (!to) return <>{children}</>;
  return (
    <button
      type="button"
      onClick={() => go(to)}
      className="text-left hover:underline"
      style={{ color: "var(--color-accent)" }}
    >
      {children}
    </button>
  );
}

/** Inline word-level from→to rendering: deletions struck through in danger,
 *  additions in success, unchanged text muted. memo: the LCS is quadratic
 *  and unrelated dialog state (rename input) re-renders the tree. */
const InlineWordDiff = memo(function InlineWordDiff({
  from,
  to,
}: {
  from: string;
  to: string;
}) {
  return (
    <>
      {wordDiff(from, to).map((r, i) => (
        <span
          key={i}
          style={
            r.kind === "del"
              ? {
                  color: "var(--color-danger)",
                  textDecoration: "line-through",
                }
              : r.kind === "add"
                ? { color: "var(--color-success)" }
                : { color: "var(--color-muted)" }
          }
        >
          {r.text}
        </span>
      ))}
    </>
  );
});
