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
