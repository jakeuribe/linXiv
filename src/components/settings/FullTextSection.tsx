import { useQuery, useQueryClient } from "@tanstack/react-query";
import { getSettings, updateSettings } from "../../api/settings";
import { Toggle } from "../ui/toggle";
import { SettingGroup, SettingGroupLabel, SettingRow } from "./SettingRow";

export function FullTextSection() {
  const qc = useQueryClient();
  const { data: settings } = useQuery({
    queryKey: ["settings"],
    queryFn: getSettings,
  });

  const enabled =
    (settings as Record<string, unknown> | undefined)?.full_text_worker_enabled === true;

  function handleToggle(next: boolean) {
    updateSettings({ full_text_worker_enabled: next })
      .then(() => qc.invalidateQueries({ queryKey: ["settings"] }))
      .catch(console.error);
  }

  return (
    <div>
      <SettingGroupLabel>Full-text indexing</SettingGroupLabel>
      <p className="mb-2.5 text-xs text-muted">
        Downloads the arXiv TeX source of papers that aren&rsquo;t indexed yet, one
        at a time, so full-text search covers your whole library. It respects
        arXiv&rsquo;s rate limits, so a large library takes hours — it just keeps
        working in the background until it&rsquo;s done. Papers it can&rsquo;t fetch
        (non-arXiv, or a failed download) are retried after a restart.
      </p>
      <SettingGroup>
        <SettingRow
          label="Index full text in the background"
          description="Off by default: each paper is a multi-megabyte download"
        >
          <Toggle
            checked={enabled}
            onChange={handleToggle}
            aria-label="Index full text in the background"
          />
        </SettingRow>
      </SettingGroup>
    </div>
  );
}
