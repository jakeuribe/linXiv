// Reading-list model: a reading list IS a project carrying the reserved
// READING_LIST_TAG (the single source of truth on both sides — the backend's
// is_reading_list_project derives from the same tag). Per-paper read state
// lives in the backend PAPER_TO_READING table, reached via api/readingStatus.

export const READING_LIST_TAG = "reading-list";

export function isReadingListProject(p: { project_tags: string[] }): boolean {
  return p.project_tags.some(t => t.toLowerCase() === READING_LIST_TAG);
}

/** Unread is the absence of a record; only deviations are stored. */
export type ReadingStatus = "reading" | "read";

/** Click-to-cycle transition: unread → reading → read → unread. */
export function cycleStatus(
  cur: ReadingStatus | undefined
): ReadingStatus | undefined {
  if (cur === undefined) return "reading";
  if (cur === "reading") return "read";
  return undefined;
}

/** Salvage statuses from the retired zustand-persist localStorage blob
 * (`{"state":{"statuses":{...}},"version":n}`). Garbage in any shape — bad
 * JSON, wrong nesting, invalid values — yields {} / drops the entry, never
 * throws: this feeds the one-time push to the backend. */
export function parsePersistedReadingStatuses(
  raw: string | null
): Record<string, ReadingStatus> {
  if (raw === null) return {};
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return {};
  }
  const statuses = (parsed as { state?: { statuses?: unknown } })?.state?.statuses;
  if (typeof statuses !== "object" || statuses === null) return {};
  const out: Record<string, ReadingStatus> = {};
  for (const [sid, status] of Object.entries(statuses)) {
    if (status === "reading" || status === "read") out[sid] = status;
  }
  return out;
}

/** Push legacy entries to the backend via `put` (which swallows skippable
 * failures itself, e.g. a paper that no longer exists). Resolves true when
 * every entry went through, false when any push threw — the caller keeps the
 * blob so an idempotent retry can finish the job later. */
export async function pushLegacyStatuses(
  entries: Record<string, ReadingStatus>,
  put: (sourceId: string, status: ReadingStatus) => Promise<unknown>
): Promise<boolean> {
  let allOk = true;
  for (const [sid, status] of Object.entries(entries)) {
    try {
      await put(sid, status);
    } catch {
      allOk = false;
    }
  }
  return allOk;
}

export function statusLabel(cur: ReadingStatus | undefined): string {
  return cur === "read" ? "Read" : cur === "reading" ? "Reading" : "Unread";
}

/** Queue = papers on any reading list not yet read; "reading" sorts first. */
export function queueOf<T extends { source_id: string }>(
  papers: T[],
  listSourceIds: ReadonlySet<string>,
  statuses: Record<string, ReadingStatus>
): T[] {
  return papers
    .filter(
      (p) => listSourceIds.has(p.source_id) && statuses[p.source_id] !== "read"
    )
    .sort(
      (a, b) =>
        Number(statuses[b.source_id] === "reading") -
        Number(statuses[a.source_id] === "reading")
    );
}
