import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { getSettings, updateSettings } from "../../api/settings";
import { Input } from "../ui/input";
import { Toggle } from "../ui/toggle";
import { SettingGroup, SettingGroupLabel, SettingRow } from "./SettingRow";

export function SearchSection() {
  const { data: settings } = useQuery({
    queryKey: ["settings"],
    queryFn: getSettings,
  });

  const historyEnabled = settings?.search_history_enabled !== false;
  const historyMax = settings?.search_history_max ?? 200;

  const [maxInput, setMaxInput] = useState("200");
  const [populated, setPopulated] = useState(false);
  if (settings && !populated) {
    setMaxInput(String(historyMax));
    setPopulated(true);
  }

  function handleToggle(next: boolean) {
    updateSettings({ search_history_enabled: next }).catch(console.error);
  }

  function handleMaxBlur() {
    const n = parseInt(maxInput, 10);
    if (!isNaN(n) && n > 0) {
      updateSettings({ search_history_max: n }).catch(console.error);
    } else {
      setMaxInput(String(historyMax));
    }
  }

  return (
    <div>
      <SettingGroupLabel>Search</SettingGroupLabel>
      <SettingGroup>
        <SettingRow
          label="Search history"
          description="Save clause terms for autocomplete suggestions"
        >
          <Toggle
            checked={historyEnabled}
            onChange={handleToggle}
            aria-label="Search history"
          />
        </SettingRow>
        <SettingRow
          label="Max history entries"
          description="Oldest terms are pruned when the limit is reached"
          className={historyEnabled ? "" : "opacity-40"}
        >
          <Input
            type="number"
            min={1}
            max={10000}
            value={maxInput}
            onChange={(e) => setMaxInput(e.target.value)}
            onBlur={handleMaxBlur}
            disabled={!historyEnabled}
            className="w-24 text-right"
          />
        </SettingRow>
      </SettingGroup>
    </div>
  );
}
