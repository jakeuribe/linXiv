import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  createShareTicket,
  joinShare,
  listReceived,
  sharingAvailable,
  type SharedSummary,
} from "../../api/share";
import { listProjects } from "../../api/projects";
import { ApiError } from "../../api/client";
import { Button } from "../ui/button";
import { Textarea } from "../ui/input";
import { OptionSelect } from "../ui/select";
import { Spinner } from "../ui/spinner";
import { SettingGroup, SettingGroupLabel, SettingRow } from "./SettingRow";

function errText(e: unknown): string {
  return e instanceof ApiError ? e.message : "Unexpected sharing error";
}

function summaryLine(s: SharedSummary): string {
  return `${s.paper_count} papers · ${s.note_count} notes · ${s.tag_count} tags`;
}

export function SharingSection() {
  const queryClient = useQueryClient();
  const [selected, setSelected] = useState("");
  const [ticket, setTicket] = useState("");
  const [generating, setGenerating] = useState(false);
  const [shareErr, setShareErr] = useState("");
  const [copied, setCopied] = useState(false);

  const [joinInput, setJoinInput] = useState("");
  const [joining, setJoining] = useState(false);
  const [joinErr, setJoinErr] = useState("");
  // set when a join was accepted but its host was offline: the invite is saved
  // and the share appears once it syncs, so this is the only feedback the user
  // gets that anything happened.
  const [joinPending, setJoinPending] = useState("");

  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  // Same query keys as SharePage, so a share/join done on either surface
  // refreshes both through the one react-query cache.
  const { data: projectsData } = useQuery({
    queryKey: ["projects", "active"],
    queryFn: () => listProjects("active"),
    enabled: sharingAvailable,
  });
  const projects = projectsData?.projects ?? [];
  const { data: received = [] } = useQuery({
    queryKey: ["share", "received"],
    queryFn: listReceived,
    enabled: sharingAvailable,
  });

  if (!sharingAvailable) {
    return (
      <div>
        <SettingGroupLabel>Sharing</SettingGroupLabel>
        <SettingGroup>
          <SettingRow
            label="Project sharing"
            description="Peer-to-peer sharing runs over the desktop app's network node and isn't available in the browser preview."
          />
        </SettingGroup>
      </div>
    );
  }

  async function handleGenerate() {
    const id = Number(selected);
    if (!id) return;
    setGenerating(true);
    setShareErr("");
    setTicket("");
    setCopied(false);
    try {
      const t = await createShareTicket(id);
      // Minting a ticket also publishes the project — refresh the Hoster grid.
      queryClient.invalidateQueries({ queryKey: ["share", "published"] });
      if (alive.current) setTicket(t);
    } catch (e) {
      if (alive.current) setShareErr(errText(e));
    } finally {
      if (alive.current) setGenerating(false);
    }
  }

  async function handleCopy() {
    try {
      await navigator.clipboard.writeText(ticket);
      if (alive.current) {
        setCopied(true);
        setTimeout(() => alive.current && setCopied(false), 1500);
      }
    } catch {
      // Clipboard denied: the ticket is still selectable in the textarea.
    }
  }

  async function handleJoin() {
    const t = joinInput.trim();
    if (!t) return;
    setJoining(true);
    setJoinErr("");
    setJoinPending("");
    try {
      const res = await joinShare(t);
      queryClient.invalidateQueries({ queryKey: ["share", "received"] });
      if (alive.current) {
        setJoinInput("");
        if (res.pending) setJoinPending(res.reason);
      }
    } catch (e) {
      if (alive.current) setJoinErr(errText(e));
    } finally {
      if (alive.current) setJoining(false);
    }
  }

  return (
    <div>
      <SettingGroupLabel>Sharing</SettingGroupLabel>
      <SettingGroup>
        <SettingRow
          label="Share a project"
          description="Generate a ticket, then paste it on another computer to send a read-only copy."
        >
          <OptionSelect
            aria-label="Project to share"
            size="sm"
            placeholder="Select a project…"
            value={selected}
            onChange={setSelected}
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
        </SettingRow>

        {shareErr && (
          <SettingRow label="Error">
            <span style={{ color: "var(--color-danger)" }}>{shareErr}</span>
          </SettingRow>
        )}

        {ticket && (
          <div className="flex flex-col gap-2 py-3">
            <div className="flex items-center justify-between">
              <span className="text-sm font-medium text-text">Ticket</span>
              <Button variant="muted" size="sm" onClick={handleCopy}>
                {copied ? "Copied" : "Copy"}
              </Button>
            </div>
            <Textarea readOnly value={ticket} rows={3} onFocus={(e) => e.currentTarget.select()} />
          </div>
        )}
      </SettingGroup>

      <SettingGroup>
        <div className="flex flex-col gap-2 py-3">
          <div className="flex items-center justify-between">
            <span className="text-sm font-medium text-text">Join a shared project</span>
            <Button
              variant="primary"
              size="sm"
              onClick={handleJoin}
              disabled={joining || !joinInput.trim()}
            >
              {joining ? <Spinner size={14} /> : "Join"}
            </Button>
          </div>
          <Textarea
            placeholder="Paste a ticket from another computer…"
            value={joinInput}
            onChange={(e) => setJoinInput(e.target.value)}
            rows={3}
          />
          {joinErr && (
            <span className="text-xs" style={{ color: "var(--color-danger)" }}>
              {joinErr}
            </span>
          )}
          {joinPending && <span className="text-xs text-muted">{joinPending}</span>}
        </div>

        {received.map((s) => (
          <SettingRow key={s.share_id} label={s.name} description={summaryLine(s)} />
        ))}
      </SettingGroup>
    </div>
  );
}
