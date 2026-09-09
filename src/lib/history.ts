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

/** Who wrote a change: this device, the host-assigned member name, or a
 *  short peer actor id. */
export function formatActor(
  actor: string,
  mine: boolean,
  displayName?: string | null
): string {
  if (mine) return "This device";
  return displayName || actor.slice(0, 8);
}

/** The viewer's identity hexes (journal actor, p2p endpoint id), lowercased,
 *  absent values dropped. */
export function viewerIdentities(
  ...ids: Array<string | null | undefined>
): string[] {
  return ids.filter((v): v is string => !!v).map((v) => v.toLowerCase());
}

/** Viewer-true "mine": match the change's actor against the VIEWER's own
 *  identities; fall back to the serving backend's wire flag only when none
 *  are known (it wrongly marks a remote node's changes as "mine"). */
export function isMineChange(
  actor: string,
  wireMine: boolean,
  viewerIds: string[]
): boolean {
  return viewerIds.length ? viewerIds.includes(actor.toLowerCase()) : wireMine;
}

export interface WordDiffRun {
  kind: "same" | "add" | "del";
  text: string;
}

/** Word-level LCS diff over whitespace-preserving tokens: concatenating the
 *  same+del runs re-yields `from`, same+add re-yields `to`. The cell cap
 *  below bounds the O(n·m) table regardless of input size. */
export function wordDiff(from: string, to: string): WordDiffRun[] {
  const a = from.split(/(\s+)/).filter(Boolean);
  const b = to.split(/(\s+)/).filter(Boolean);
  // Bail out of the quadratic table on oversized inputs (degenerate walls of
  // single-char tokens, or un-clipped long text). Normal clip-size prose is
  // ~700 tokens/side (~500k cells) and stays word-level.
  if (a.length * b.length > 1_000_000) {
    const runs: WordDiffRun[] = [];
    if (from) runs.push({ kind: "del", text: from });
    if (to) runs.push({ kind: "add", text: to });
    return runs;
  }
  // dp[i][j] = LCS length of a[i..] vs b[j..].
  const dp: number[][] = Array.from({ length: a.length + 1 }, () =>
    new Array<number>(b.length + 1).fill(0)
  );
  for (let i = a.length - 1; i >= 0; i--) {
    for (let j = b.length - 1; j >= 0; j--) {
      dp[i][j] =
        a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
    }
  }
  const runs: WordDiffRun[] = [];
  const push = (kind: WordDiffRun["kind"], text: string) => {
    const last = runs[runs.length - 1];
    if (last && last.kind === kind) last.text += text;
    else runs.push({ kind, text });
  };
  let i = 0;
  let j = 0;
  while (i < a.length && j < b.length) {
    if (a[i] === b[j]) {
      push("same", a[i]);
      i++;
      j++;
    } else if (dp[i + 1][j] >= dp[i][j + 1]) push("del", a[i++]);
    else push("add", b[j++]);
  }
  while (i < a.length) push("del", a[i++]);
  while (j < b.length) push("add", b[j++]);
  return runs;
}

/** Change timestamp; pre-timestamp changes (0) render as an em dash. */
export function formatTime(unixSecs: number): string {
  if (!unixSecs) return "—";
  return new Date(unixSecs * 1000).toLocaleString(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  });
}
