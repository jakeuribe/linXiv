import { useIsMutating, useMutation, useQueryClient } from "@tanstack/react-query";
import { syncShare, type SharedSummary } from "../../api/share";
import { latestSyncedAt, SHARE_SYNC_MUTATION_KEY } from "../../lib/syncPill";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
import { Spinner } from "../ui/spinner";
import { relAgo } from "./ShareCard";

/** Global sync-state pill for the Shared projects header: "Syncing changes…"
 * while any share sync (card-level or sync-all) is in flight, otherwise
 * "All changes synced" with the most recent synced_at across all shares. */
export function SyncStatusPill({ shares }: { shares: SharedSummary[] }) {
  const queryClient = useQueryClient();
  const syncing = useIsMutating({ mutationKey: SHARE_SYNC_MUTATION_KEY }) > 0;
  const syncAll = useMutation({
    mutationKey: SHARE_SYNC_MUTATION_KEY,
    mutationFn: async () => {
      // ponytail: sequential, one node behind them all; parallelize if share
      // counts ever make this slow.
      let failed = 0;
      for (const s of shares) {
        try {
          await syncShare(s.share_id);
        } catch {
          failed++; // the card-level sync surfaces per-share detail
        }
      }
      return { failed };
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ["share", "published"] });
      queryClient.invalidateQueries({ queryKey: ["share", "received"] });
    },
  });
  const latest = latestSyncedAt(shares);
  return (
    <div className="flex items-center gap-2">
      <Badge className="gap-1.5 font-mono text-[10.5px] font-semibold">
        {syncing ? (
          <>
            <Spinner size={11} /> Syncing changes…
          </>
        ) : (
          <>
            <span
              className="h-1.5 w-1.5 rounded-full"
              style={{ backgroundColor: "var(--color-success)" }}
            />
            All changes synced{latest && ` · Last sync ${relAgo(latest)}`}
          </>
        )}
      </Badge>
      {syncAll.data && syncAll.data.failed > 0 && (
        <span className="text-xs" style={{ color: "var(--color-danger)" }}>
          {syncAll.data.failed} share{syncAll.data.failed === 1 ? "" : "s"}{" "}
          failed to sync
        </span>
      )}
      <Button
        variant="muted"
        size="sm"
        onClick={() => syncAll.mutate()}
        disabled={syncing || shares.length === 0}
      >
        Sync now
      </Button>
    </div>
  );
}
