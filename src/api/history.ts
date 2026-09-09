import { apiFetch } from "./client";
import { libraryFetch } from "../stores/backend.ts";
import type {
  DeviceActor,
  HistoryDiff,
  RestoredToChange,
  Timeline,
} from "../types/api";

/** What a history view is scoped to. Shares are log/diff only — restoring a
 *  shared project goes through its project scope. */
export type HistoryScope =
  | { kind: "library" }
  | { kind: "project"; id: number }
  | { kind: "share"; shareId: string };

function base(scope: HistoryScope): string {
  switch (scope.kind) {
    case "library":
      return "/api/history/library";
    case "project":
      return `/api/history/project/${scope.id}`;
    case "share":
      return `/api/history/share/${encodeURIComponent(scope.shareId)}`;
  }
}

export async function getTimeline(scope: HistoryScope): Promise<Timeline> {
  return libraryFetch<Timeline>(base(scope));
}

/** THIS device's journal actor — always local (apiFetch), so "mine" reflects
 *  the viewer even when the timeline came from a remote node. */
export async function getDeviceActor(): Promise<DeviceActor> {
  return apiFetch<DeviceActor>("/api/history/actor");
}

/** What one change did: state at its parents vs state at the change. */
export async function getChangeDiff(
  scope: HistoryScope,
  at: string
): Promise<HistoryDiff> {
  return libraryFetch<HistoryDiff>(
    `${base(scope)}/diff?at=${encodeURIComponent(at)}`
  );
}

export async function restoreTo(
  scope: Exclude<HistoryScope, { kind: "share" }>,
  to: string
): Promise<RestoredToChange> {
  return libraryFetch(`${base(scope)}/restore`, {
    method: "POST",
    body: JSON.stringify({ to }),
  });
}
