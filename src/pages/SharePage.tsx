import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  createShareTicket,
  joinShare,
  listReceived,
  listShared,
  sharingAvailable,
  type SharedSummary,
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

function ShareCard({ share, role }: { share: SharedSummary; role: ShareRole }) {
  const hosted = role === "Hoster";
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
          className="h-1.5 w-1.5 shrink-0 animate-pulse rounded-full"
          style={{ backgroundColor: "var(--color-success)" }}
        />
        <span className="truncate text-xs" style={{ color: "var(--color-muted)" }}>
          {hosted
            ? "Published from your library — tickets grant a read-only copy."
            : "Read-only mirror — fetched from a collaborator's ticket."}
        </span>
      </div>
      <div className="flex items-center gap-4 px-5 pb-4 pt-3">
        <Stat value={share.paper_count} label="paper" />
        <Stat value={share.note_count} label="note" />
        <Stat value={share.tag_count} label="tag" />
      </div>
    </div>
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
  });
  const { isError: publishedIsError, error: publishedError } = published;
  const received = useQuery({
    queryKey: ["share", "received"],
    queryFn: listReceived,
    enabled: sharingAvailable,
  });
  const { isError: receivedIsError, error: receivedError } = received;

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
            <ShareCard key={`${role}:${share.share_id}`} share={share} role={role} />
          ))}
        </div>
      )}

      <ShareProjectDialog open={dialogOpen} onClose={() => setDialogOpen(false)} />
    </div>
  );
}
