import { useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { getSettings, updateSettings } from "../../api/settings";
import { Input } from "../ui/input";
import { Spinner } from "../ui/spinner";
import { SettingGroup, SettingGroupLabel, SettingRow } from "./SettingRow";

export function HomeFeedSection() {
  const queryClient = useQueryClient();
  const requestRef = useRef(0);
  const { data: settings, isLoading: settingsLoading, isError: settingsError } = useQuery({
    queryKey: ["settings"],
    queryFn: getSettings,
  });

  const saved =
    typeof settings?.home_feed_url === "string" ? settings.home_feed_url : "";

  const [input, setInput] = useState("");
  const [prevSaved, setPrevSaved] = useState<string | null>(null);
  const [error, setError] = useState("");
  if (settings && saved !== prevSaved) {
    setInput(saved);
    setPrevSaved(saved);
  }

  function handleBlur() {
    setError("");
    const next = input.trim();
    if (next === saved) {
      setInput(next);
      return;
    }
    if (next !== "" && !/^https?:\/\//i.test(next)) {
      setError("Must be http:// or https://");
      return;
    }
    const thisRequest = ++requestRef.current;
    updateSettings({ home_feed_url: next })
      .then(() => {
        if (thisRequest === requestRef.current) {
          setInput(next);
          queryClient.invalidateQueries({ queryKey: ["home-feed"] });
        }
      })
      .catch(() => {
        if (thisRequest === requestRef.current) {
          setError("Failed to save");
          setInput(saved);
        }
      });
  }

  return (
    <div>
      <SettingGroupLabel>Home</SettingGroupLabel>
      <SettingGroup>
        <SettingRow
          label="Home feed URL"
          description="RSS/Atom feed shown on the home page, e.g. https://rss.arxiv.org/rss/cs.LG — leave empty for the default dashboard"
          descriptionId="home-feed-url-desc"
        >
          {settingsLoading ? (
            <span className="flex items-center gap-2 text-sm text-muted">
              <Spinner size={14} /> Loading…
            </span>
          ) : settingsError ? (
            <span className="text-xs text-danger">Could not load settings.</span>
          ) : (
            <div className="flex flex-col gap-2">
              <Input
                type="url"
                value={input}
                onChange={(e) => {
                  setInput(e.target.value);
                  setError("");
                }}
                onBlur={handleBlur}
                placeholder="https://rss.arxiv.org/rss/cs.LG"
                aria-label="Home feed URL"
                aria-describedby={error ? "home-feed-url-desc home-feed-url-error" : "home-feed-url-desc"}
                aria-invalid={!!error}
                className="w-80"
              />
              {error && (
                <div id="home-feed-url-error" className="text-sm text-danger">
                  {error}
                </div>
              )}
            </div>
          )}
        </SettingRow>
      </SettingGroup>
    </div>
  );
}
