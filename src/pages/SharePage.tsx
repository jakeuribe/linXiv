import { useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Settings2 } from "lucide-react";
import {
  createShareTicket,
  getShareSettings,
  importReceived,
  joinShare,
  leaveShare,
  listReceived,
  listShared,
  sharingAvailable,
  syncShare,
  unpublishShare,
  updateShareSettings,
  type ShareDirection,
  type SharedSummary,
  type ShareSettings,
} from "../api/share";
import { listProjects } from "../api/projects";
import { ApiError } from "../api/client";
import { Button } from "../components/ui/button";
import { Dialog } from "../components/ui/dialog";
import { Input, Textarea } from "../components/ui/input";
import { OptionSelect } from "../components/ui/select";
import { Spinner } from "../components/ui/spinner";

// UI derived from the linXiv.dc.html design mock's Shared view, grounded to the
// Phase-0 share backend: one-shot ticket publish/join, no presence/members yet.

type ShareRole = "Hoster" | "Reader";

function errText(e: unknown): string {
  return e instanceof ApiError ? e.message : "Something went wrong";
}

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

/** "Synced 5m ago" from the summary's ISO synced_at. */
function syncedText(iso: string | null): string {
  if (!iso) return "Never synced";
  const mins = Math.floor((Date.now() - new Date(iso).getTime()) / 60_000);
  if (!Number.isFinite(mins)) return "Synced";
  if (mins < 1) return "Synced just now";
  if (mins < 60) return `Synced ${mins}m ago`;
  if (mins < 24 * 60) return `Synced ${Math.floor(mins / 60)}h ago`;
  return `Synced ${Math.floor(mins / (24 * 60))}d ago`;
}

const SYNC_REASON_LABELS: Record<string, string | undefined> = {
  "no ticket": "No valid ticket",
  "p2p offline": "P2P offline",
  "project gone": "Project deleted",
  "bad ticket": "Bad ticket",
  "paused": "Sync paused",
  "direction": "Skipped by sync direction",
};

function humanizeReason(code: string | undefined): string {
  if (!code) return "Sync failed";
  return SYNC_REASON_LABELS[code] ?? code;
}

function ShareCard({
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
  const sync = useMutation({
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
            {share.name}
          </span>
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
          {share.paused ? "Sync paused" : syncedText(share.synced_at)}
          {" · "}
          {hosted ? "published from your library" : "read-only mirror"}
        </span>
      </div>
      <div className="flex items-center gap-4 px-5 pb-4 pt-3">
        <Stat value={share.paper_count} label="paper" />
        <Stat value={share.note_count} label="note" />
        <Stat value={share.tag_count} label="tag" />
        <div className="flex-1" />
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
      {(sync.isError || sync.data?.synced === false) && (
        <p className="px-5 pb-3 text-xs" style={{ color: "var(--color-danger)" }}>
          {sync.isError ? errText(sync.error) : humanizeReason(sync.data?.reason)}
        </p>
      )}
    </div>
  );
}

const DIRECTION_OPTIONS: { value: ShareDirection; label: string }[] = [
  { value: "two_way", label: "Two-way" },
  { value: "shared_to_local", label: "Shared → local only" },
  { value: "local_to_shared", label: "Local → shared only" },
];

function SettingsRow({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-3">
      <span className="text-[13px] text-text">{label}</span>
      {children}
    </div>
  );
}

function ShareSettingsDialog({
  share,
  role,
  onClose,
}: {
  share: SharedSummary;
  role: ShareRole;
  onClose: () => void;
}) {
  const hosted = role === "Hoster";
  const queryClient = useQueryClient();
  const [confirming, setConfirming] = useState(false);

  const settings = useQuery({
    queryKey: ["share", "settings", share.share_id],
    queryFn: () => getShareSettings(share.share_id),
  });
  // Resolves the hoster's project and the reader's linked-project name
  // by matching against both active and archived projects.
  const projectsActiveQ = useQuery({
    queryKey: ["projects", "active"],
    queryFn: () => listProjects("active"),
  });
  const projectsArchivedQ = useQuery({
    queryKey: ["projects", "archived"],
    queryFn: () => listProjects("archived"),
  });
  const projects = [
    ...(projectsActiveQ.data?.projects ?? []),
    ...(projectsArchivedQ.data?.projects ?? []),
  ];
  const hosterProject = hosted
    ? projects.find((p) => p.share_id === share.share_id)
    : undefined;
  const linkedProject =
    !hosted && share.project_fk != null
      ? projects.find((p) => p.id === share.project_fk)
      : undefined;

  function invalidateShares() {
    queryClient.invalidateQueries({ queryKey: ["share", "published"] });
    queryClient.invalidateQueries({ queryKey: ["share", "received"] });
  }

  const update = useMutation({
    mutationFn: (patch: Partial<ShareSettings>) =>
      updateShareSettings(share.share_id, patch),
    onSuccess: (s) => {
      queryClient.setQueryData(["share", "settings", share.share_id], s);
      invalidateShares();
    },
  });
  const importM = useMutation({
    mutationFn: () => importReceived(share.share_id),
    onSuccess: () => {
      invalidateShares();
      queryClient.invalidateQueries({ queryKey: ["projects"] });
    },
  });
  const leaveM = useMutation({
    mutationFn: () => leaveShare(share.share_id),
    onSuccess: () => {
      invalidateShares();
      onClose();
    },
  });
  const unpublishM = useMutation({
    mutationFn: () => unpublishShare(share.share_id),
    onSuccess: () => {
      invalidateShares();
      onClose();
    },
  });

  const err =
    update.error ?? importM.error ?? leaveM.error ?? unpublishM.error ?? settings.error;
  const settingsUnusable = settings.isLoading || settings.isError;
  const paused = settings.data?.paused ?? share.paused;
  const dangerLabel = hosted ? "Unpublish" : "Leave share";
  const dangerPending = leaveM.isPending || unpublishM.isPending;

  return (
    <Dialog open onClose={onClose} title={`Settings — ${share.name}`}>
      <div className="flex flex-col gap-4">
        <SettingsRow label="Sync direction">
          <OptionSelect
            aria-label="Sync direction"
            size="sm"
            value={settings.data?.direction ?? "two_way"}
            onChange={(v) => update.mutate({ direction: v })}
            disabled={settingsUnusable || update.isPending}
            options={DIRECTION_OPTIONS}
          />
        </SettingsRow>
        <SettingsRow label="Auto-sync">
          <Button
            variant="muted"
            size="sm"
            onClick={() => update.mutate({ paused: !paused })}
            disabled={settingsUnusable || update.isPending}
          >
            {paused ? "Resume sync" : "Pause sync"}
          </Button>
        </SettingsRow>
        <SettingsRow label="Local project">
          {hosted ? (
            <span className="truncate text-[13px]" style={{ color: "var(--color-muted)" }}>
              {hosterProject?.name ?? "—"}
            </span>
          ) : share.project_fk == null ? (
            <Button
              variant="primary"
              size="sm"
              onClick={() => importM.mutate()}
              disabled={importM.isPending}
            >
              {importM.isPending ? <Spinner size={14} /> : "Import to library"}
            </Button>
          ) : (
            <span className="truncate text-[13px]" style={{ color: "var(--color-muted)" }}>
              {linkedProject?.name ?? `Project #${share.project_fk}`}
            </span>
          )}
        </SettingsRow>
        {err != null && (
          <p className="text-xs" style={{ color: "var(--color-danger)" }}>
            {errText(err)}
          </p>
        )}
        <div className="flex items-center justify-between border-t border-[var(--color-border)] pt-4">
          <span className="text-xs" style={{ color: "var(--color-muted)" }}>
            {hosted
              ? "Stops serving the share. Your project stays."
              : "Removes the mirror. Imported data stays."}
          </span>
          <div className="flex items-center gap-2">
            {confirming && (
              <Button variant="muted" size="sm" onClick={() => setConfirming(false)}>
                Cancel
              </Button>
            )}
            <Button
              variant="danger"
              size="sm"
              disabled={dangerPending}
              onClick={() => {
                if (!confirming) return setConfirming(true);
                if (hosted) unpublishM.mutate();
                else leaveM.mutate();
              }}
            >
              {dangerPending ? (
                <Spinner size={14} />
              ) : confirming ? (
                `Confirm ${dangerLabel.toLowerCase()}`
              ) : (
                dangerLabel
              )}
            </Button>
          </div>
        </div>
      </div>
    </Dialog>
  );
}

function ShareProjectDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const queryClient = useQueryClient();
  const [selected, setSelected] = useState("");
  const [ticket, setTicket] = useState("");
  const [generating, setGenerating] = useState(false);
  const [error, setError] = useState("");
  const [copied, setCopied] = useState(false);
  const genTokenRef = useRef(0);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const { data } = useQuery({
    queryKey: ["projects", "active"],
    queryFn: () => listProjects("active"),
    enabled: open,
  });
  const projects = data?.projects ?? [];

  function resetTicketState() {
    genTokenRef.current++;
    setTicket("");
    setError("");
    setCopied(false);
    setGenerating(false);
  }

  async function handleGenerate() {
    const id = Number(selected);
    if (!id || generating) return;
    const token = ++genTokenRef.current;
    setGenerating(true);
    setError("");
    setTicket("");
    setCopied(false);
    try {
      const t = await createShareTicket(id);
      if (genTokenRef.current !== token || !alive.current) return;
      setTicket(t);
      // Minting a ticket also publishes the project, so the Hoster grid grows.
      queryClient.invalidateQueries({ queryKey: ["share", "published"] });
    } catch (e) {
      if (genTokenRef.current !== token || !alive.current) return;
      setError(errText(e));
    } finally {
      if (genTokenRef.current === token && alive.current) setGenerating(false);
    }
  }

  async function handleCopy() {
    try {
      await navigator.clipboard.writeText(ticket);
      if (!alive.current) return;
      setCopied(true);
      setTimeout(() => {
        if (alive.current) setCopied(false);
      }, 1500);
    } catch {
      // Clipboard denied: the ticket is still selectable in the textarea.
    }
  }

  function handleClose() {
    resetTicketState();
    setSelected("");
    onClose();
  }

  return (
    <Dialog open={open} onClose={handleClose} title="Share a project">
      <div className="flex flex-col gap-4">
        <p className="text-xs" style={{ color: "var(--color-muted)" }}>
          Generate a ticket, then paste it in linXiv on another computer to send a
          read-only copy of the project — papers, notes and tags travel with it.
        </p>
        <div className="flex items-center gap-2">
          <OptionSelect
            aria-label="Project to share"
            size="sm"
            placeholder="Select a project…"
            value={selected}
            onChange={(v) => {
              setSelected(v);
              resetTicketState();
            }}
            options={projects.map((p) => ({ value: String(p.id), label: p.name }))}
          />
          <Button
            variant="primary"
            size="sm"
            onClick={handleGenerate}
            disabled={generating || !selected}
          >
            {generating ? <Spinner size={14} /> : "Create ticket"}
          </Button>
        </div>
        {error && (
          <p className="text-xs" style={{ color: "var(--color-danger)" }}>
            {error}
          </p>
        )}
        {ticket && (
          <div className="flex flex-col gap-2">
            <div className="flex items-center justify-between">
              <span
                className="font-mono text-[10.5px] font-semibold uppercase tracking-[0.08em]"
                style={{ color: "var(--color-ink-3)" }}
              >
                Invite ticket
              </span>
              <Button variant="muted" size="sm" onClick={handleCopy}>
                {copied ? "Copied" : "Copy"}
              </Button>
            </div>
            <Textarea readOnly value={ticket} rows={3} onFocus={(e) => e.currentTarget.select()} />
          </div>
        )}
      </div>
    </Dialog>
  );
}

export default function SharePage() {
  const queryClient = useQueryClient();
  const [dialogOpen, setDialogOpen] = useState(false);
  const [settingsFor, setSettingsFor] = useState<{
    shareId: string;
    role: ShareRole;
  } | null>(null);
  const [joinInput, setJoinInput] = useState("");
  const [joining, setJoining] = useState(false);
  const [joinErr, setJoinErr] = useState("");
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const published = useQuery({
    queryKey: ["share", "published"],
    queryFn: listShared,
    enabled: sharingAvailable,
    refetchInterval: 60_000,
  });
  const { isError: publishedIsError, error: publishedError } = published;
  const received = useQuery({
    queryKey: ["share", "received"],
    queryFn: listReceived,
    enabled: sharingAvailable,
    refetchInterval: 60_000,
  });
  const { isError: receivedIsError, error: receivedError } = received;
  // Reading dataUpdatedAt makes it a tracked prop: each 60s poll re-renders
  // the cards so their relative "Synced Xm ago" text stays current.
  void published.dataUpdatedAt;
  void received.dataUpdatedAt;

  useEffect(() => {
    if (!settingsFor || published.isLoading || received.isLoading) return;
    const list = settingsFor.role === "Hoster" ? published.data : received.data;
    if (!list?.some((s) => s.share_id === settingsFor.shareId)) setSettingsFor(null);
  }, [settingsFor, published.isLoading, received.isLoading, published.data, received.data]);

  if (!sharingAvailable) {
    return (
      <div className="flex h-full items-center justify-center p-8">
        <div
          className="max-w-md rounded-lg border p-5 text-center text-sm"
          style={{
            borderColor: "var(--color-border)",
            backgroundColor: "var(--color-panel)",
            color: "var(--color-muted)",
          }}
        >
          Peer-to-peer sharing runs over the desktop app's network node and isn't
          available in the browser preview.
        </div>
      </div>
    );
  }

  async function handleJoin() {
    const t = joinInput.trim();
    if (!t || joining) return;
    setJoining(true);
    setJoinErr("");
    try {
      await joinShare(t);
      if (!alive.current) return;
      setJoinInput("");
      queryClient.invalidateQueries({ queryKey: ["share", "received"] });
    } catch (e) {
      if (!alive.current) return;
      setJoinErr(errText(e));
    } finally {
      if (alive.current) setJoining(false);
    }
  }

  const loading = published.isLoading || received.isLoading;
  const cards: { share: SharedSummary; role: ShareRole }[] = [
    ...(published.data ?? []).map((s) => ({ share: s, role: "Hoster" as const })),
    ...(received.data ?? []).map((s) => ({ share: s, role: "Reader" as const })),
  ];

  return (
    <div className="flex h-full flex-col gap-6 overflow-y-auto p-8">
      {/* Header */}
      <div className="flex items-end gap-4">
        <div>
          <h1 className="font-display text-[27px] font-semibold leading-tight tracking-[-0.015em] text-text">
            Shared projects
          </h1>
          <p className="mt-1.5 text-[13px]" style={{ color: "var(--color-muted)" }}>
            Libraries you share with collaborators over peer-to-peer tickets —
            papers, notes and tags travel with the share.
          </p>
        </div>
        <div className="flex-1" />
        <Button onClick={() => setDialogOpen(true)}>Share a project</Button>
      </div>

      {/* Join bar */}
      <div className="flex items-center gap-5 rounded-lg border border-[var(--color-border)] bg-[var(--color-panel)] px-5 py-4">
        <div className="flex shrink-0 items-center gap-2.5">
          <span
            className="h-2 w-2 animate-pulse rounded-full"
            style={{ backgroundColor: "var(--color-success)" }}
          />
          <div>
            <div className="text-[13.5px] font-semibold text-text">
              Join a shared project
            </div>
            <div className="mt-0.5 font-mono text-[11px]" style={{ color: "var(--color-ink-3)" }}>
              iroh · paste a ticket from another linXiv
            </div>
          </div>
        </div>
        <Input
          aria-label="Share ticket"
          className="flex-1 font-mono"
          placeholder="Paste a ticket…"
          value={joinInput}
          onChange={(e) => setJoinInput(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && handleJoin()}
        />
        <Button variant="primary" onClick={handleJoin} disabled={joining || !joinInput.trim()}>
          {joining ? <Spinner size={14} /> : "Join"}
        </Button>
      </div>
      {joinErr && (
        <p className="-mt-4 text-xs" style={{ color: "var(--color-danger)" }}>
          {joinErr}
        </p>
      )}

      {(publishedIsError || receivedIsError) && (
        <div
          className="rounded-lg border p-4 text-sm"
          style={{
            borderColor: "var(--color-danger)",
            color: "var(--color-danger)",
            backgroundColor: "var(--color-panel)",
          }}
        >
          Failed to load shared projects:{" "}
          {[publishedIsError && errText(publishedError), receivedIsError && errText(receivedError)]
            .filter(Boolean)
            .join("; ")}
        </div>
      )}

      {/* Grid */}
      {loading && (
        <div className="flex flex-1 items-center justify-center">
          <Spinner size={28} />
        </div>
      )}
      {!loading && cards.length === 0 && !publishedIsError && !receivedIsError && (
        <div
          className="flex flex-1 items-center justify-center text-sm"
          style={{ color: "var(--color-muted)" }}
        >
          No shared projects yet — share one of yours or join with a ticket.
        </div>
      )}
      {!loading && cards.length > 0 && (
        <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
          {cards.map(({ share, role }) => (
            <ShareCard
              key={`${role}:${share.share_id}`}
              share={share}
              role={role}
              onSettings={() => setSettingsFor({ shareId: share.share_id, role })}
            />
          ))}
        </div>
      )}

      <ShareProjectDialog open={dialogOpen} onClose={() => setDialogOpen(false)} />
      {settingsFor &&
        (() => {
          const card = cards.find(
            c => c.share.share_id === settingsFor.shareId && c.role === settingsFor.role
          );
          if (!card) return null;
          return (
            <ShareSettingsDialog
              share={card.share}
              role={card.role}
              onClose={() => setSettingsFor(null)}
            />
          );
        })()}
    </div>
  );
}
