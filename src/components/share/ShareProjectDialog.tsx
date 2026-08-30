import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { createShareTicket, publishSecure, shareErrText } from "../../api/share";
import { listProjects } from "../../api/projects";
import { Button } from "../ui/button";
import { Dialog } from "../ui/dialog";
import { Textarea } from "../ui/input";
import { OptionSelect } from "../ui/select";
import { Spinner } from "../ui/spinner";

const SHARE_MODE_OPTIONS: { value: "plain" | "e2ee"; label: string }[] = [
  { value: "plain", label: "Plain ticket" },
  { value: "e2ee", label: "End-to-end encrypted" },
];

export function ShareProjectDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
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
      setError(shareErrText(e));
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
        <div className="flex flex-wrap items-center gap-2">
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
