import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { getSettings, updateEnv } from "../../api/settings";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { SettingGroup, SettingGroupLabel, SettingRow } from "./SettingRow";

export function CrossRefSection() {
  const { data: settings } = useQuery({
    queryKey: ["settings"],
    queryFn: getSettings,
  });

  const [crossrefEmail, setCrossrefEmail] = useState("");
  const [populated, setPopulated] = useState(false);
  if (settings && !populated) {
    if (typeof (settings as Record<string, unknown>)["CROSSREF_MAILTO"] === "string") {
      setCrossrefEmail((settings as Record<string, unknown>)["CROSSREF_MAILTO"] as string);
    }
    setPopulated(true);
  }

  return (
    <div>
      <SettingGroupLabel>CrossRef</SettingGroupLabel>
      <SettingGroup>
        <SettingRow
          label="Contact email"
          description={
            <>
              Used as the <code className="text-accent">mailto</code> parameter for
              polite CrossRef API access.
            </>
          }
        >
          <Input
            type="email"
            value={crossrefEmail}
            onChange={(e) => setCrossrefEmail(e.target.value)}
            placeholder="you@example.com"
            aria-label="CrossRef contact email"
            style={{ maxWidth: 320 }}
          />
          <Button
            size="sm"
            onClick={() => updateEnv("CROSSREF_MAILTO", crossrefEmail).catch(console.error)}
          >
            Save
          </Button>
        </SettingRow>
      </SettingGroup>
    </div>
  );
}
