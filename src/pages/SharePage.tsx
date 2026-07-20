import { useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Lock, Settings2 } from "lucide-react";
import {
  createShareTicket,
  downloadSharedPdf,
  getShareSettings,
  importReceived,
  inviteMember,
  joinShare,
  leaveShare,
  listMembers,
  listReceived,
  listReceivedPapers,
  listShared,
  memberCode,
  publishSecure,
  revokeMember,
  setMemberRole,
  sharingAvailable,
  syncShare,
  unpublishShare,
  updateShareSettings,
  type ShareDirection,
  type SharedSummary,
  type ShareMember,
  type ShareSettings,
} from "../api/share";
import { listProjects } from "../api/projects";
import { ApiError } from "../api/client";
import { Button } from "../components/ui/button";
import { Dialog } from "../components/ui/dialog";
import { Input, Textarea } from "../components/ui/input";
import { OptionSelect } from "../components/ui/select";
import { Spinner } from "../components/ui/spinner";

// UI derived from the linXiv.dc.html design mock's Shared view: plain ticket
// publish/join plus e2ee shares (per-member invites, PDF blobs); no presence.

type ShareRole = "Hoster" | "Reader";

function errText(e: unknown): string {
  return e instanceof ApiError ? e.message : "Unexpected sharing error";
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
  "revoked or awaiting key": "Access revoked or key not yet received",
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
          failed.push(`${p.title || p.source_id}: ${errText(e)}`);
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
      queryClient.invalidateQueries({ queryKey: ["papers"] });
      queryClient.invalidateQueries({ queryKey: ["paper"] });
    },
  });
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
        <p className="px-5 pb-3 text-xs" style={{ color: "var(--color-danger)" }}>
          {sync.isError ? errText(sync.error) : humanizeReason(sync.data?.reason)}
        </p>
      )}
      {pdfs.isError && (
        <p className="px-5 pb-3 text-xs" style={{ color: "var(--color-danger)" }}>
          {errText(pdfs.error)}
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

const INVITE_ROLE_OPTIONS: { value: "editor" | "viewer"; label: string }[] = [
  { value: "viewer", label: "Viewer" },
  { value: "editor", label: "Editor" },
];

/** Hoster-only e2ee members panel: sidecar list + invite + revoke. */
function MembersSection({ shareId }: { shareId: string }) {
  const queryClient = useQueryClient();
  const [code, setCode] = useState("");
  const [role, setRole] = useState<"editor" | "viewer">("viewer");
  const [name, setName] = useState("");
  const [invite, setInvite] = useState("");
  const [copied, setCopied] = useState(false);
  const [revoking, setRevoking] = useState<string | null>(null);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const membersQ = useQuery({
    queryKey: ["share", "members", shareId],
    queryFn: () => listMembers(shareId),
  });
  function invalidateMembers() {
    queryClient.invalidateQueries({ queryKey: ["share", "members", shareId] });
    queryClient.invalidateQueries({ queryKey: ["share", "published"] });
  }
  const inviteM = useMutation({
    mutationFn: () =>
      inviteMember(shareId, {
        memberCode: code.trim(),
        role,
        name: name.trim() || undefined,
      }),
    onSuccess: (inv) => {
      setInvite(inv);
      setCopied(false);
      setCode("");
      setName("");
      invalidateMembers();
    },
  });
  const revokeM = useMutation({
    mutationFn: (memberId: string) => revokeMember(shareId, memberId),
    onSuccess: () => {
      setRevoking(null);
      invalidateMembers();
    },
  });
  // §3.3 viewer ↔ editor dropdown: optimistic flip, rolled back on error
  // (the error line below is the page's toast equivalent).
  const roleM = useMutation({
    mutationFn: ({ memberId, role }: { memberId: string; role: "editor" | "viewer" }) =>
      setMemberRole(shareId, memberId, role),
    onMutate: async ({ memberId, role }) => {
      await queryClient.cancelQueries({ queryKey: ["share", "members", shareId] });
      const prev = queryClient.getQueryData<ShareMember[]>(["share", "members", shareId]);
      queryClient.setQueryData<ShareMember[]>(["share", "members", shareId], (list) =>
        list?.map((m) => (m.member_id === memberId ? { ...m, role } : m))
      );
      return { prev };
    },
    onError: (_e, _v, ctx) => {
      if (ctx?.prev) queryClient.setQueryData(["share", "members", shareId], ctx.prev);
    },
    onSettled: () => invalidateMembers(),
  });

  async function handleCopy() {
    try {
      await navigator.clipboard.writeText(invite);
      if (!alive.current) return;
      setCopied(true);
      setTimeout(() => {
        if (alive.current) setCopied(false);
      }, 1500);
    } catch {
      // Clipboard write denied.
    }
  }

  const members = membersQ.data ?? [];
  return (
    <div className="flex flex-col gap-3 border-t border-[var(--color-border)] pt-4">
      <span
        className="font-mono text-[10.5px] font-semibold uppercase tracking-[0.08em]"
        style={{ color: "var(--color-ink-3)" }}
      >
        Members
      </span>
      {membersQ.isLoading && <Spinner size={16} />}
      {members.map((m) => (
        <div key={m.member_id || m.invited_at} className="flex items-center gap-2">
          <span
            className="flex-1 truncate text-[13px]"
            style={{ color: m.revoked ? "var(--color-ink-3)" : "var(--color-text)" }}
          >
            {m.name ||
              (m.role === "hoster"
                ? "This device"
                : m.member_id.slice(0, 8) || "unknown device")}
          </span>
          {!m.revoked && m.role !== "hoster" && m.verified && m.member_id ? (
            // Viewer/Editor only — co-admin is keyhive-supported but
            // app-deferred (spec §1.1); the route refuses admin targets.
            <OptionSelect
              aria-label={`Role for ${m.name || m.member_id.slice(0, 8)}`}
              size="sm"
              value={m.role as "editor" | "viewer"}
              onChange={(r) => roleM.mutate({ memberId: m.member_id, role: r })}
              disabled={roleM.isPending}
              options={INVITE_ROLE_OPTIONS}
            />
          ) : (
            <span
              className="shrink-0 rounded-full border border-[var(--color-border)] bg-[var(--color-surface-2)] px-2 py-0.5 font-mono text-[10px] font-semibold"
              style={{ color: "var(--color-muted)" }}
            >
              {m.revoked ? "revoked" : m.verified ? m.role : `${m.role} (unverified)`}
            </span>
          )}
          {!m.revoked && m.role !== "hoster" && revoking === m.member_id && (
            <Button
              variant="muted"
              size="sm"
              disabled={revokeM.isPending}
              onClick={() => setRevoking(null)}
            >
              Cancel
            </Button>
          )}
          {!m.revoked && m.role !== "hoster" && (
            <Button
              variant={revoking === m.member_id ? "danger" : "ghost"}
              size="sm"
              disabled={revokeM.isPending}
              onClick={() => {
                if (revoking !== m.member_id) return setRevoking(m.member_id);
                inviteM.reset();
                revokeM.mutate(m.member_id);
              }}
            >
              {revokeM.isPending && revoking === m.member_id ? (
                <Spinner size={14} />
              ) : revoking === m.member_id ? (
                "Confirm revoke"
              ) : (
                "Revoke"
              )}
            </Button>
          )}
        </div>
      ))}
      {revoking != null && (
        <p className="text-xs" style={{ color: "var(--color-muted)" }}>
          Stops receiving future updates. Content already synced stays on their
          device.
        </p>
      )}
      <div className="flex items-center gap-2">
        <Input
          aria-label="Member code"
          className="flex-1 font-mono"
          placeholder="Paste their member code…"
          value={code}
          onChange={(e) => setCode(e.target.value)}
        />
        <OptionSelect
          aria-label="Invite role"
          size="sm"
          value={role}
          onChange={setRole}
          options={INVITE_ROLE_OPTIONS}
        />
      </div>
      <div className="flex items-center gap-2">
        <Input
          aria-label="Member name"
          className="flex-1"
          placeholder="Name (optional)"
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        <Button
          variant="primary"
          size="sm"
          onClick={() => {
            revokeM.reset();
            inviteM.mutate();
          }}
          disabled={inviteM.isPending || !code.trim()}
        >
          {inviteM.isPending ? <Spinner size={14} /> : "Invite"}
        </Button>
      </div>
      {(inviteM.isError || revokeM.isError || roleM.isError || membersQ.isError) && (
        <p className="text-xs" style={{ color: "var(--color-danger)" }}>
          {errText(inviteM.error ?? revokeM.error ?? roleM.error ?? membersQ.error)}
        </p>
      )}
      {invite && (
        <div className="flex flex-col gap-2">
          <div className="flex items-center justify-between">
            <span
              className="font-mono text-[10.5px] font-semibold uppercase tracking-[0.08em]"
              style={{ color: "var(--color-ink-3)" }}
            >
              Invite string — send it to them
            </span>
            <Button variant="muted" size="sm" onClick={handleCopy}>
              {copied ? "Copied" : "Copy"}
            </Button>
          </div>
          <Textarea
            readOnly
            value={invite}
            rows={3}
            onFocus={(e) => e.currentTarget.select()}
          />
        </div>
      )}
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
        {share.e2ee && (
          <div
            className="flex items-center gap-2 text-xs"
            style={{ color: "var(--color-muted)" }}
          >
            <Lock size={13} />
            End-to-end encrypted · syncs every 5 minutes
          </div>
        )}
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
        {hosted && share.e2ee && <MembersSection shareId={share.share_id} />}
        <div className="flex items-center justify-between border-t border-[var(--color-border)] pt-4">
          <span className="text-xs" style={{ color: "var(--color-muted)" }}>
            {hosted
              ? share.e2ee
                ? "Revokes all members and stops serving the share. Your project stays."
                : "Stops serving the share. Your project stays."
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

const SHARE_MODE_OPTIONS: { value: "plain" | "e2ee"; label: string }[] = [
  { value: "plain", label: "Plain ticket" },
  { value: "e2ee", label: "End-to-end encrypted" },
];

function ShareProjectDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const queryClient = useQueryClient();
  const [selected, setSelected] = useState("");
  const [mode, setMode] = useState<"plain" | "e2ee">("plain");
  const [ticket, setTicket] = useState("");
  const [secured, setSecured] = useState(false);
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
    setSecured(false);
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
    setSecured(false);
    setCopied(false);
    try {
      if (mode === "e2ee") {
        await publishSecure(id);
        if (genTokenRef.current !== token || !alive.current) return;
        setSecured(true);
      } else {
        const t = await createShareTicket(id);
        if (genTokenRef.current !== token || !alive.current) return;
        setTicket(t);
      }
      // Publishing (either mode) grows the Hoster grid.
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
          {mode === "e2ee"
            ? "Publish an encrypted copy of the project. There is no ticket. You may generate an invite string for each member from the share's settings using their member code."
            : "Generate a ticket, then paste it in linXiv on another computer to send a read-only copy of the project."}
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
          <OptionSelect
            aria-label="Share mode"
            size="sm"
            value={mode}
            onChange={(v) => {
              setMode(v);
              resetTicketState();
            }}
            options={SHARE_MODE_OPTIONS}
          />
          <Button
            variant="primary"
            size="sm"
            onClick={handleGenerate}
            disabled={generating || !selected}
          >
            {generating ? (
              <Spinner size={14} />
            ) : mode === "e2ee" ? (
              "Publish encrypted"
            ) : (
              "Create ticket"
            )}
          </Button>
        </div>
        {error && (
          <p className="text-xs" style={{ color: "var(--color-danger)" }}>
            {error}
          </p>
        )}
        {secured && (
          <p className="text-xs" style={{ color: "var(--color-muted)" }}>
            Published encrypted. Open the share's settings to invite members —
            each device sends you its member code first.
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
  const [codeCopied, setCodeCopied] = useState(false);
  const [codeErr, setCodeErr] = useState("");
  const [myCode, setMyCode] = useState("");
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
    setCodeErr("");
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

  async function handleCopyMemberCode() {
    setCodeErr("");
    setJoinErr("");
    try {
      const code = await memberCode();
      if (!alive.current) return;
      setMyCode(code);
      try {
        await navigator.clipboard.writeText(code);
        if (!alive.current) return;
        setCodeCopied(true);
        setTimeout(() => {
          if (alive.current) setCodeCopied(false);
        }, 1500);
      } catch {
        // Clipboard write denied.
      }
    } catch (e) {
      if (alive.current) setCodeErr(errText(e));
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
      <div className="flex flex-col gap-2.5 rounded-lg border border-[var(--color-border)] bg-[var(--color-panel)] px-5 py-4">
        <div className="flex items-center gap-5">
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
                iroh · paste a ticket or invite from another linXiv
              </div>
            </div>
          </div>
          <Input
            aria-label="Share ticket or invite"
            className="flex-1 font-mono"
            placeholder="Paste a ticket or invite…"
            value={joinInput}
            onChange={(e) => setJoinInput(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && handleJoin()}
          />
          <Button variant="primary" onClick={handleJoin} disabled={joining || !joinInput.trim()}>
            {joining ? <Spinner size={14} /> : "Join"}
          </Button>
        </div>
        <div className="flex items-center gap-2">
          <Button variant="muted" size="sm" onClick={handleCopyMemberCode}>
            {codeCopied ? "Copied" : "Your member code"}
          </Button>
          <span className="text-[11px]" style={{ color: "var(--color-ink-3)" }}>
            Send this to a host to be invited to an encrypted share.
          </span>
        </div>
        {myCode && (
          <Textarea
            readOnly
            aria-label="Your member code"
            className="font-mono"
            value={myCode}
            rows={2}
            onFocus={(e) => e.currentTarget.select()}
          />
        )}
      </div>
      {(joinErr || codeErr) && (
        <p className="-mt-4 text-xs" style={{ color: "var(--color-danger)" }}>
          {joinErr || codeErr}
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
          No shared projects yet. Press Create Shared Project to share one of yours, or join with a ticket provided by another linXiv user.
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
