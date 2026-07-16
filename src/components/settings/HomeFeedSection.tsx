import { useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { getSettings, updateSettings } from "../../api/settings";
import { createFeedRule, deleteFeedRule, listFeedRules } from "../../api/feed";
import type { FeedFilterRule } from "../../types/api";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { OptionSelect } from "../ui/select";
import { Spinner } from "../ui/spinner";
import { SettingGroup, SettingGroupLabel, SettingRow } from "./SettingRow";

const FILTER_FIELDS: FeedFilterRule["field"][] = ["TITLE", "SUMMARY", "AUTHOR"];
const FILTER_ACTIONS: FeedFilterRule["action"][] = ["DENY", "ALLOW"];

function FeedFilterRulesSection() {
  const queryClient = useQueryClient();
  const { data: rules, isLoading } = useQuery({
    queryKey: ["feed-rules"],
    queryFn: listFeedRules,
  });
  const [field, setField] = useState<FeedFilterRule["field"]>("TITLE");
  const [action, setAction] = useState<FeedFilterRule["action"]>("DENY");
  const [keywords, setKeywords] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  function invalidate() {
    queryClient.invalidateQueries({ queryKey: ["feed-rules"] });
    queryClient.invalidateQueries({ queryKey: ["home-feed"] });
  }

  async function handleAdd() {
    const trimmed = keywords.trim();
    if (trimmed === "") return;
    setBusy(true);
    setError("");
    try {
      await createFeedRule(field, trimmed, action);
      setKeywords("");
      invalidate();
    } catch (err) {
      console.error(err);
      setError("Failed to add rule");
    } finally {
      setBusy(false);
    }
  }

  async function handleDelete(ruleId: number) {
    setBusy(true);
    setError("");
    try {
      await deleteFeedRule(ruleId);
      invalidate();
    } catch (err) {
      console.error(err);
      setError("Failed to remove rule");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div>
      <SettingGroupLabel>Feed filters</SettingGroupLabel>
      <SettingGroup block>
        <p className="mb-3 text-xs text-muted">
          Auto-hide home feed entries. DENY rules hide a match (comma-separated
          keywords all must appear); an ALLOW rule overrides a DENY match.
        </p>
        <div className="flex flex-wrap items-center gap-2">
          <OptionSelect
            aria-label="Field"
            options={FILTER_FIELDS.map((f) => ({ value: f, label: f }))}
            value={field}
            onChange={setField}
            size="sm"
          />
          <OptionSelect
            aria-label="Action"
            options={FILTER_ACTIONS.map((a) => ({ value: a, label: a }))}
            value={action}
            onChange={setAction}
            size="sm"
          />
          <Input
            value={keywords}
            onChange={(e) => setKeywords(e.target.value)}
            placeholder="keyword, another keyword"
            aria-label="Keywords"
            className="w-64"
          />
          <Button size="sm" disabled={busy || keywords.trim() === ""} onClick={handleAdd}>
            Add rule
          </Button>
        </div>
        {error !== "" && (
          <p className="mt-2 text-xs" style={{ color: "var(--color-danger)" }}>
            {error}
          </p>
        )}
        {isLoading ? (
          <div className="mt-3 flex items-center gap-2 text-sm text-muted">
            <Spinner size={14} /> Loading…
          </div>
        ) : rules !== undefined && rules.length > 0 ? (
          <ul className="mt-3 flex flex-col gap-1.5">
            {rules.map((rule) => (
              <li
                key={rule.rule_id}
                className="flex items-center justify-between gap-3 text-xs text-text"
              >
                <span>
                  <strong>{rule.action}</strong> {rule.field}: {rule.keywords}
                </span>
                <button
                  type="button"
                  className="text-muted hover:text-text transition-colors"
                  disabled={busy}
                  onClick={() => handleDelete(rule.rule_id)}
                >
                  Remove
                </button>
              </li>
            ))}
          </ul>
        ) : (
          <p className="mt-3 text-xs text-muted">No filter rules yet.</p>
        )}
      </SettingGroup>
    </div>
  );
}

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
      <div className="mt-6">
        <FeedFilterRulesSection />
      </div>
    </div>
  );
}
