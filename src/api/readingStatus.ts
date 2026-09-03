import { ApiError } from "./client.ts";
import { libraryFetch } from "../stores/backend.ts";
import {
  parsePersistedReadingStatuses,
  pushLegacyStatuses,
  type ReadingStatus,
} from "../lib/readingStatus.ts";

/** Query key for the global source_id → status map. Backend rows are keyed per
 * (reading list, paper); PUT fans one write out to every list the paper is on
 * and GET aggregates back to one status per paper — see
 * `crates/core/src/service/reading_list.rs` for the keying contract. */
export const READING_STATUS_QUERY_KEY = ["reading-status"];

export async function getReadingStatuses(): Promise<{
  statuses: Record<string, ReadingStatus>;
}> {
  return libraryFetch("/api/reading-status");
}

/** `applied` is how many reading lists were written; 0 means the paper is on
 * none (a no-op, not an error). */
export async function putReadingStatus(
  sourceId: string,
  status: ReadingStatus | "unread"
): Promise<{ ok: boolean; applied: number }> {
  return libraryFetch(`/api/reading-status/${encodeURIComponent(sourceId)}`, {
    method: "PUT",
    body: JSON.stringify({ status }),
  });
}

/** The retired zustand-persist blob (store deleted when persistence moved to
 * the backend). Its presence marks the one-time migration as still owed. */
const LEGACY_STORAGE_KEY = "linxiv-reading-status";

/** One-time migration: push any statuses persisted by the old localStorage
 * store into the backend, then retire the blob. Papers that no longer exist
 * are skipped (404 → their status is unrecoverable anyway); papers on no
 * reading list apply as a 0-list no-op. Any other failure keeps the blob so
 * the next fetch retries — the PUTs are idempotent. Never throws. */
export async function migrateLegacyLocalStatuses(): Promise<void> {
  const raw = localStorage.getItem(LEGACY_STORAGE_KEY);
  if (raw === null) return;
  const entries = parsePersistedReadingStatuses(raw);
  const allOk = await pushLegacyStatuses(entries, async (sid, status) => {
    try {
      await putReadingStatus(sid, status);
    } catch (err) {
      if (err instanceof ApiError && err.status === 404) return; // paper gone
      throw err;
    }
  });
  if (allOk) localStorage.removeItem(LEGACY_STORAGE_KEY);
}

/** queryFn for READING_STATUS_QUERY_KEY: run the legacy migration (a no-op
 * once the blob is gone), then fetch the map. */
export async function fetchReadingStatuses(): Promise<
  Record<string, ReadingStatus>
> {
  await migrateLegacyLocalStatuses();
  return (await getReadingStatuses()).statuses;
}
