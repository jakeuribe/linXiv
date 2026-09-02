import type { SharedSummary, SyncReason } from "../api/share";

/** Every ShareCard sync and the pill's sync-all register under this key, so
 * `useIsMutating` can answer "is any share sync in flight?" globally. */
export const SHARE_SYNC_MUTATION_KEY = ["share", "sync"];

/** `syncShare` resolves (does not throw) for most failures, reporting them as
 * `{ synced: false, reason }`. A resolved result counts as failed unless it
 * synced or was intentionally skipped ("paused" / "direction" — the only
 * non-failure reasons in `SyncReason`). */
export function isSyncFailure(r: { synced: boolean; reason?: SyncReason }): boolean {
  if (r.synced) return false;
  return r.reason !== "paused" && r.reason !== "direction";
}

/** Most recent `synced_at` across all listed shares, or null when none has
 * ever synced. Feeds the pill's "Last sync 5m ago" text. */
export function latestSyncedAt(shares: SharedSummary[]): string | null {
  let latest: string | null = null;
  for (const s of shares) {
    if (s.synced_at == null) continue;
    const t = new Date(s.synced_at).getTime();
    if (!Number.isFinite(t)) continue;
    if (latest == null || t > new Date(latest).getTime()) latest = s.synced_at;
  }
  return latest;
}
