import { useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  inviteMember,
  listMembers,
  rekeyShare,
  removeMember,
  revokeMember,
  setMemberRole,
  type ShareMember,
  shareErrText,
} from "../../api/share";
import { Button } from "../ui/button";
import { Input, Textarea } from "../ui/input";
import { OptionSelect } from "../ui/select";
import { Spinner } from "../ui/spinner";
import { relAgo } from "./ShareCard";

const INVITE_ROLE_OPTIONS: { value: "editor" | "viewer"; label: string }[] = [
  { value: "viewer", label: "Viewer" },
  { value: "editor", label: "Editor" },
];

/** Hoster-only e2ee members panel: sidecar list + invite + revoke. */
export function MembersSection({ shareId }: { shareId: string }) {
  const queryClient = useQueryClient();
  const [code, setCode] = useState("");
  const [role, setRole] = useState<"editor" | "viewer">("viewer");
  const [name, setName] = useState("");
  const [invite, setInvite] = useState("");
  // Key of whatever was copied last: "new" for the freshly minted string, or a
  // member id for a re-sent one.
  const [copied, setCopied] = useState<string | null>(null);
  const [revoking, setRevoking] = useState<string | null>(null);
  const [removing, setRemoving] = useState<string | null>(null);
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
      setCopied(null);
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
  const rekeyM = useMutation({
    mutationFn: () => rekeyShare(shareId),
    onSuccess: invalidateMembers,
  });
  // The rollback arm: revoke + drop the row, so re-inviting the same device
  // does not inherit a stale role or a dead invite string.
  const removeM = useMutation({
    mutationFn: (memberId: string) => removeMember(shareId, memberId),
    onSuccess: () => {
      setRemoving(null);
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

  async function handleCopy(key: string, text: string) {
    try {
      await navigator.clipboard.writeText(text);
      if (!alive.current) return;
      setCopied(key);
      setTimeout(() => {
        if (alive.current) setCopied((c) => (c === key ? null : c));
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
      {members.some((m) => m.role !== "hoster" && !m.revoked) && (
        <div className="flex items-center gap-2">
          <span className="flex-1 text-[11px]" style={{ color: "var(--color-ink-3)" }}>
            {rekeyM.isSuccess
              ? `Re-keyed for ${rekeyM.data.members} member${rekeyM.data.members === 1 ? "" : "s"}. They should sync again.`
              : "If a member syncs but stays empty, re-key: their invite came after this content was encrypted."}
          </span>
          <Button
            variant="muted"
            size="sm"
            onClick={() => rekeyM.mutate()}
            disabled={rekeyM.isPending}
          >
            {rekeyM.isPending ? <Spinner size={14} /> : "Re-key"}
          </Button>
        </div>
      )}
      {rekeyM.isError && (
        <p className="text-xs" style={{ color: "var(--color-danger)" }}>
          {shareErrText(rekeyM.error)}
        </p>
      )}
      {members.map((m) => (
        <div key={m.member_id || m.invited_at} className="flex flex-col gap-1">
          <div className="flex items-center gap-2">
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
          {m.role !== "hoster" && (
            // Outstanding-invite line: a member who never picks theirs up leaves
            // no other trace on this end, and the string is otherwise shown once.
            <div className="flex items-center gap-2">
              <span className="text-[11px]" style={{ color: "var(--color-ink-3)" }}>
                Invited {relAgo(m.invited_at)}
                {m.revoked
                  ? " · revoked"
                  : m.verified
                    ? ""
                    : " · not confirmed by the key layer yet"}
              </span>
              <div className="flex-1" />
              {!m.revoked && m.invite && (
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => handleCopy(m.member_id, m.invite as string)}
                >
                  {copied === m.member_id ? "Copied" : "Copy invite"}
                </Button>
              )}
              {removing === m.member_id && (
                <Button
                  variant="muted"
                  size="sm"
                  disabled={removeM.isPending}
                  onClick={() => setRemoving(null)}
                >
                  Cancel
                </Button>
              )}
              {m.member_id && (
                <Button
                  variant={removing === m.member_id ? "danger" : "ghost"}
                  size="sm"
                  disabled={removeM.isPending}
                  onClick={() => {
                    if (removing !== m.member_id) return setRemoving(m.member_id);
                    inviteM.reset();
                    removeM.mutate(m.member_id);
                  }}
                >
                  {removeM.isPending && removing === m.member_id ? (
                    <Spinner size={14} />
                  ) : removing === m.member_id ? (
                    "Confirm remove"
                  ) : (
                    "Remove"
                  )}
                </Button>
              )}
            </div>
          )}
        </div>
      ))}
      {revoking != null && (
        <p className="text-xs" style={{ color: "var(--color-muted)" }}>
          Stops receiving future updates. Content already synced stays on their
          device.
        </p>
      )}
      {removing != null && (
        <p className="text-xs" style={{ color: "var(--color-muted)" }}>
          Revokes them and forgets the invite entirely, so re-inviting the same
          device starts clean. They should also leave the share on their end.
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
          {shareErrText(inviteM.error ?? revokeM.error ?? roleM.error ?? membersQ.error)}
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
            <Button variant="muted" size="sm" onClick={() => handleCopy("new", invite)}>
              {copied === "new" ? "Copied" : "Copy"}
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
