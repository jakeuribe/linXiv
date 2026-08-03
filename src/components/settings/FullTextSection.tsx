import { useMutation, useQuery } from "@tanstack/react-query";
import { getSettings, updateSettings } from "../../api/settings";
import { fullTextPending } from "../../api/papers";
import { Toggle } from "../ui/toggle";
import { SettingGroup, SettingGroupLabel, SettingRow } from "./SettingRow";

export function FullTextSection() {
  const { data: settings, isLoading } = useQuery({
    queryKey: ["settings"],
    queryFn: getSettings,
  });

  const enabled = settings?.full_text_worker_enabled === true;

  // While the worker runs, the backlog is the only signal that it is making
  // progress, so poll it; idle otherwise.
  const { data: backlog } = useQuery({
    queryKey: ["papers", "full-text-pending"],
    queryFn: fullTextPending,
    refetchInterval: enabled ? 30_000 : false,
  });

  const toggleMutation = useMutation({
    mutationFn: (next: boolean) => updateSettings({ full_text_worker_enabled: next }),
  });

  return (
    <div>
      <SettingGroupLabel>Full-text indexing</SettingGroupLabel>
      <p className="mb-2.5 text-xs text-muted">
        Downloads the TeX source of arXiv papers that aren&rsquo;t indexed yet, one
        at a time, so full-text search reaches inside them. It respects
        arXiv&rsquo;s rate limits, so a large library takes hours — it just keeps
        working in the background until it&rsquo;s done. Papers from other sources
        have no source to fetch and are skipped; ones that fail are retried later.
      </p>
      <SettingGroup>
        <SettingRow
          label="Index full text in the background"
          description="Off by default: each paper is a multi-megabyte download, stored in your library database"
        >
          <Toggle
            checked={enabled}
            onChange={(next) => toggleMutation.mutate(next)}
            disabled={isLoading || toggleMutation.isPending}
            aria-label="Index full text in the background"
          />
        </SettingRow>
        {backlog !== undefined && (
          <p className="py-2 text-sm text-muted">
            {backlog.pending === 0
              ? "Every paper is indexed."
              : `${backlog.pending} paper${backlog.pending === 1 ? "" : "s"} not indexed yet.`}
          </p>
        )}
      </SettingGroup>
      {toggleMutation.isError && (
        <p className="mt-2 text-xs text-danger">
          Could not save the setting. Check that the data directory is writable.
        </p>
      )}
    </div>
  );
}
