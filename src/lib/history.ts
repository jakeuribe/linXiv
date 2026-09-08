import type { HistoryDiff } from "../types/api";

/** git --stat-style one-liner: "+2 papers · −1 note · ~1 annotation". Empty
 *  string for a no-op diff. */
export function diffSummary(d: HistoryDiff): string {
  const parts: string[] = [];
  const push = (n: number, sign: string, noun: string) => {
    if (n > 0) parts.push(`${sign}${n} ${noun}${n === 1 ? "" : "s"}`);
  };
  push(d.papers_added.length, "+", "paper");
  push(d.papers_removed.length, "−", "paper");
  push(d.tags_added.length, "+", "tag");
  push(d.tags_removed.length, "−", "tag");
  push(d.notes_added.length, "+", "note");
  push(d.notes_changed.length, "~", "note");
  push(d.notes_removed.length, "−", "note");
  push(d.annotations_added.length, "+", "annotation");
  push(d.annotations_changed.length, "~", "annotation");
  push(d.annotations_removed.length, "−", "annotation");
  if (d.meta.length > 0) parts.push(`~${d.meta.map((m) => m.field).join(", ")}`);
  return parts.join(" · ");
}

/** Who wrote a change: this device, or a short peer actor id. */
export function formatActor(actor: string, mine: boolean): string {
  return mine ? "This device" : actor.slice(0, 8);
}

/** Change timestamp; pre-timestamp changes (0) render as an em dash. */
export function formatTime(unixSecs: number): string {
  if (!unixSecs) return "—";
  return new Date(unixSecs * 1000).toLocaleString(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  });
}
