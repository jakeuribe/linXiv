// Reading-list model: a reading list IS a project carrying the reserved
// READING_LIST_TAG; per-paper read state lives in stores/readingStatus.

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

/** Merge-papers support: move `fromId`'s status onto `toId` unless the winner
 * already carries one (winner wins — mirrors the backend merge), dropping the
 * loser's entry either way. Returns the input object untouched when there is
 * nothing to migrate. */
export function migrateStatus(
  statuses: Record<string, ReadingStatus>,
  fromId: string,
  toId: string
): Record<string, ReadingStatus> {
  if (!(fromId in statuses)) return statuses;
  const next = { ...statuses };
  if (next[toId] === undefined) next[toId] = next[fromId];
  delete next[fromId];
  return next;
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
