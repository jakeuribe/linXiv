import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { getSettings, updateEnv } from "../../api/settings";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { Section } from "./Section";

export function OpenAlexSection({ defaultOpen = true }: { defaultOpen?: boolean } = {}) {
  const { data: settings } = useQuery({
    queryKey: ["settings"],
    queryFn: getSettings,
  });

  const [openalexEmail, setOpenalexEmail] = useState("");
  const [saveStatus, setSaveStatus] = useState<"idle" | "saved" | "error">("idle");
  const [populated, setPopulated] = useState(false);
  useEffect(() => {
    if (settings && !populated) {
      if (typeof (settings as Record<string, unknown>)["OPENALEX_MAILTO"] === "string") {
        setOpenalexEmail((settings as Record<string, unknown>)["OPENALEX_MAILTO"] as string);
      }
      setPopulated(true);
    }
  }, [settings, populated]);

  return (
    <Section title="OpenAlex" defaultOpen={defaultOpen}>
      <div className="flex flex-col gap-1 mb-2">
        <label className="text-sm text-muted font-medium">Contact Email</label>
        <p className="text-xs text-muted mb-2">
          Sent as a{" "}
          <code className="text-accent">mailto</code> address for polite-pool OpenAlex API access.
        </p>
        <div className="flex gap-2 items-center">
          <Input
            type="email"
            value={openalexEmail}
            onChange={(e) => setOpenalexEmail(e.target.value)}
            placeholder="you@example.com"
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
        </div>
      </div>
    </Section>
  );
}
