import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  joinShare,
  JOIN_SLOW_HINT,
  listReceived,
  listShared,
  memberCode,
  sharingAvailable,
  type SharedSummary,
  shareErrText,
} from "../api/share";
import { useSlowHint } from "../hooks/useSlowHint";
import { Button } from "../components/ui/button";
import { Input, Textarea } from "../components/ui/input";
import { Spinner } from "../components/ui/spinner";
import { ShareCard, type ShareRole } from "../components/share/ShareCard";
import { ShareSettingsDialog } from "../components/share/ShareSettingsDialog";
import { ShareProjectDialog } from "../components/share/ShareProjectDialog";

// UI derived from the linXiv.dc.html design mock's Shared view: plain ticket
// publish/join plus e2ee shares (per-member invites, PDF blobs); no presence.

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
  // set when a join was accepted but its host was offline: the invite is saved
  // and the share only appears in the received list once it syncs, so this is
  // the only feedback the user gets that anything happened.
  const [joinPending, setJoinPending] = useState("");
  const joinSlow = useSlowHint(joining);
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
    setJoinPending("");
    try {
      const res = await joinShare(t);
      if (!alive.current) return;
      setJoinInput("");
      if (res.pending) setJoinPending(res.reason);
      queryClient.invalidateQueries({ queryKey: ["share", "received"] });
    } catch (e) {
      if (!alive.current) return;
      setJoinErr(shareErrText(e));
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
      if (alive.current) setCodeErr(shareErrText(e));
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
            Libraries you share with collaborators over peer-to-peer tickets ;
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
      {joinSlow && <p className="-mt-4 text-xs text-muted">{JOIN_SLOW_HINT}</p>}
      {joinPending && <p className="-mt-4 text-xs text-muted">{joinPending}</p>}

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
          {[publishedIsError && shareErrText(publishedError), receivedIsError && shareErrText(receivedError)]
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
