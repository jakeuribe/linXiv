import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { getSettings, updateEnv } from "../../api/settings";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { SettingGroup, SettingGroupLabel, SettingRow } from "./SettingRow";

export function OpenAlexSection() {
  const { data: settings } = useQuery({
    queryKey: ["settings"],
    queryFn: getSettings,
  });

  const [openalexEmail, setOpenalexEmail] = useState("");
  const [saveStatus, setSaveStatus] = useState<"idle" | "saved" | "error">("idle");
  const [populated, setPopulated] = useState(false);
  useEffect(() => {
    if (settings && !populated) {
      if (settings.OPENALEX_MAILTO) {
        setOpenalexEmail(settings.OPENALEX_MAILTO);
      }
      setPopulated(true);
    }
  }, [settings, populated]);

  return (
    <div>
      <SettingGroupLabel>OpenAlex</SettingGroupLabel>
      <SettingGroup>
        <SettingRow
          label="Contact email"
          description={
            <>
              Sent as a <code className="text-accent">mailto</code> address for
              polite-pool OpenAlex API access.
            </>
          }
        >
          <Input
            type="email"
            value={openalexEmail}
            onChange={(e) => setOpenalexEmail(e.target.value)}
            placeholder="you@example.com"
            aria-label="OpenAlex contact email"
            style={{ maxWidth: 320 }}
          />
          <Button
            size="sm"
            onClick={() => {
              setSaveStatus("idle");
              updateEnv("OPENALEX_MAILTO", openalexEmail)
                .then(() => setSaveStatus("saved"))
                .catch((err) => {
                  console.error(err);
                  setSaveStatus("error");
                });
            }}
          >
            Save
          </Button>
          {saveStatus === "saved" && <span className="text-xs text-success">Saved</span>}
          {saveStatus === "error" && (
            <span className="text-xs text-danger">Failed to save</span>
          )}
        </SettingRow>
      </SettingGroup>
    </div>
  );
}
