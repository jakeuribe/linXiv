import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Lock } from "lucide-react";
import {
  getShareSettings,
  leaveShare,
  unlinkShare,
  unpublishShare,
  updateShareSettings,
  type ShareDirection,
  type SharedSummary,
  type ShareSettings,
  shareErrText,
} from "../../api/share";
import { listProjects } from "../../api/projects";
import { invalidateProjectMutationQueries } from "../../lib/paperMutations";
import { Button } from "../ui/button";
import { Dialog } from "../ui/dialog";
import { OptionSelect } from "../ui/select";
import { Spinner } from "../ui/spinner";
import { useImportReceived, type ShareRole } from "./ShareCard";
import { MembersSection } from "./MembersSection";

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

export function ShareSettingsDialog({
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
  const importM = useImportReceived(share.share_id);
  const unlinkM = useMutation({
    mutationFn: () => unlinkShare(share.share_id),
    onSuccess: () => {
      invalidateShares();
      invalidateProjectMutationQueries(queryClient);
    },
  });
  const leaveM = useMutation({
    mutationFn: () => leaveShare(share.share_id),
    onSuccess: (res) => {
      invalidateShares();
      // A partial undo has to be said out loud: the mirror is gone but the p2p
      // registration is not, so a rejoin would resurrect the same stuck doc.
      if (res.forgotten === false) return;
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
    update.error ??
    importM.error ??
    unlinkM.error ??
    leaveM.error ??
    unpublishM.error ??
    settings.error;
  const settingsUnusable = settings.isLoading || settings.isError;
  const paused = settings.data?.paused ?? share.paused;
  const dangerLabel = hosted ? "Unpublish" : "Leave share";
  const dangerPending = leaveM.isPending || unpublishM.isPending;

  return (
    <Dialog
      open
      onClose={onClose}
      title={`Settings — ${share.name || "pending share"}`}
    >
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
          ) : share.pending ? (
            // Nothing has arrived to import yet; "Sync now" on the card is the
            // only useful action until the host answers.
            <span className="truncate text-[13px]" style={{ color: "var(--color-muted)" }}>
              Waiting for the first sync
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
            <div className="flex min-w-0 items-center gap-2">
              <span
                className="truncate text-[13px]"
                style={{ color: "var(--color-muted)" }}
              >
                {linkedProject?.name ?? `Project #${share.project_fk}`}
              </span>
              {/* Detaches the link only — membership, mirror, and the local
                  project all stay; the row flips back to "Import to library". */}
              <Button
                variant="muted"
                size="sm"
                title="Unlink local project"
                onClick={() => unlinkM.mutate()}
                disabled={unlinkM.isPending}
              >
                {unlinkM.isPending ? <Spinner size={14} /> : "Unlink"}
              </Button>
            </div>
          )}
        </SettingsRow>
        {err != null && (
          <p className="text-xs" style={{ color: "var(--color-danger)" }}>
            {shareErrText(err)}
          </p>
        )}
        {leaveM.data?.forgotten === false && (
          <p className="text-xs" style={{ color: "var(--color-danger)" }}>
            The mirror is gone, but the peer-to-peer node was offline so its
            registration survived — rejoining now would reuse the same document.
            Start the app with networking available and leave again.
          </p>
        )}
        {hosted && share.e2ee && <MembersSection shareId={share.share_id} />}
        <div className="flex items-center justify-between border-t border-[var(--color-border)] pt-4">
          <span className="text-xs" style={{ color: "var(--color-muted)" }}>
            {hosted
              ? share.e2ee
                ? "Revokes all members and stops serving the share. Your project stays."
                : "Stops serving the share. Your project stays."
              : share.e2ee
                ? "Removes the mirror and forgets the share, so a rejoin starts fresh. Imported data stays."
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
