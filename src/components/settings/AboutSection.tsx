import { useEffect, useRef, useState } from "react";
import { useLocation } from "react-router";
import { useQuery } from "@tanstack/react-query";
import { isTauri } from "../../api/client";
import { getSettings, updateSettings } from "../../api/settings";
import {
  checkForUpdates,
  getCurrentVersion,
  getLinuxPackageKind,
  installUpdate,
  openReleaseUrl,
  type LinuxPackageKind,
  type UpdateResult,
} from "../../api/updates";
import {
  ABOUT_GROUP_ID,
  isFrequency,
  UPDATE_CHECK_QUERY_KEY,
  UPDATE_FREQUENCIES,
  type UpdateFrequency,
} from "../../lib/updateSchedule";
import { Button } from "../ui/button";
import { OptionSelect } from "../ui/select";
import { Spinner } from "../ui/spinner";
import { SettingGroup, SettingGroupLabel, SettingRow } from "./SettingRow";
import { errText } from "../../lib/errText";

/** How long a check's answer is treated as current, for both refetching and
 *  display. */
const UPDATE_CHECK_TTL_MS = 60 * 60_000;

function AutoCheckControl() {
  // isPending, not isLoading: a query paused by the offline manager reports
  // neither loading nor error, and would fall through to "Could not load".
  const { data: settings, isPending, isError } = useQuery({
    queryKey: ["settings"],
    queryFn: getSettings,
  });
  const requestRef = useRef(0);
  const [saveError, setSaveError] = useState(false);
  // Held until the PATCH + refetch round-trip lands, so the select doesn't
  // snap back to its previous value (or to the placeholder) in between.
  const [pending, setPending] = useState<UpdateFrequency | null>(null);

  if (isPending) {
    return (
      <span className="flex items-center gap-2 text-sm text-muted">
        <Spinner size={14} /> Loading…
      </span>
    );
  }
  if (isError || settings === undefined) {
    return <span className="text-xs text-danger">Could not load settings.</span>;
  }

  function handleChange(next: UpdateFrequency | "") {
    if (!isFrequency(next)) return;
    setSaveError(false);
    setPending(next);
    const thisRequest = ++requestRef.current;
    updateSettings({ update_check_frequency: next })
      // Cleared either way: `updateSettings` awaits the refetch, so by now the
      // stored value is authoritative and holding the override would mask a
      // write from elsewhere (the launch prompt writes the same key).
      .then(() => {
        if (thisRequest === requestRef.current) setPending(null);
      })
      .catch(() => {
        if (thisRequest !== requestRef.current) return;
        setPending(null);
        setSaveError(true);
      });
  }

  // An unanswered user has no stored frequency. Showing "Never" for that reads
  // as an answer they never gave, and re-picking an already-selected option
  // fires no change event — leaving the launch prompt unanswerable from here.
  const stored = settings.update_check_frequency;
  const shown = pending ?? (isFrequency(stored) ? stored : "");

  return (
    <div className="flex items-center gap-2">
      <OptionSelect
        aria-label="Check for updates automatically"
        aria-describedby="update-frequency-desc"
        options={UPDATE_FREQUENCIES}
        value={shown}
        placeholder={shown === "" ? "Not set" : undefined}
        onChange={handleChange}
        size="sm"
      />
      {saveError && <span className="text-xs text-danger">Failed to save</span>}
    </div>
  );
}

function UpdateMessage({
  result,
  packageKind,
  packageKindResolved,
  onInstall,
  installing,
  installError,
}: {
  result: UpdateResult;
  packageKind: LinuxPackageKind | null;
  packageKindResolved: boolean;
  onInstall: () => void;
  installing: boolean;
  installError: string | null;
}) {
  // A result carrying an error compared nothing; falling through would report
  // "You're on the latest version" for a check that never completed.
  if (result.error) return null;
  if (result.hasUpdate && result.latest) {
    return (
      <span className="flex items-center gap-3 flex-wrap">
        <span style={{ color: "var(--color-success)" }}>
          Version {result.latest} is available.
        </span>
        {isTauri && (
          <Button
            variant="primary"
            size="sm"
            onClick={onInstall}
            disabled={installing || !packageKindResolved}
          >
            {installing ? (
              <>
                <Spinner size={14} /> Installing…
              </>
            ) : (
              "Install and restart"
            )}
          </Button>
        )}
        <Button
          variant={isTauri ? "muted" : "primary"}
          size="sm"
          onClick={() => openReleaseUrl(result.releaseUrl).catch(console.error)}
        >
          Download
        </Button>
        {installError && (
          <span style={{ color: "var(--color-danger)" }}>
            {installError}
            {packageKind && " (requires accepting the authentication prompt)"}
          </span>
        )}
      </span>
    );
  }
  if (result.latest === null) {
    return <span className="text-muted">No published releases yet.</span>;
  }
  if (result.current === null) {
    return (
      <span className="flex items-center gap-3 flex-wrap">
        <span className="text-muted">Latest release: {result.latest}.</span>
        <Button
          variant="muted"
          size="sm"
          onClick={() => openReleaseUrl(result.releaseUrl).catch(console.error)}
        >
          View
        </Button>
      </span>
    );
  }
  return <span style={{ color: "var(--color-success)" }}>You're on the latest version.</span>;
}

export function AboutSection() {
  const { hash } = useLocation();
  const [version, setVersion] = useState<string | null>(null);
  const [versionResolved, setVersionResolved] = useState(false);
  const [packageKind, setPackageKind] = useState<LinuxPackageKind | null>(null);
  // A seeded result paints Install on the first render, before the IPC hop
  // that says which install path applies; a native-package click before then would
  // route through the AppImage updater.
  const [packageKindResolved, setPackageKindResolved] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [installError, setInstallError] = useState<string | null>(null);
  const alive = useRef(true);

  // Enabled only for the banner's /settings#about deep link, so opening the
  // tab by hand doesn't fetch. A failed check rejects rather than resolving
  // with `error`, which keeps it out of the cache.
  const {
    data: cachedResult,
    dataUpdatedAt,
    error: checkError,
    isFetching: checking,
    refetch,
  } = useQuery({
    queryKey: [UPDATE_CHECK_QUERY_KEY],
    queryFn: async () => {
      const r = await checkForUpdates();
      if (r.error !== undefined) throw new Error(r.error);
      return r;
    },
    enabled: hash === `#${ABOUT_GROUP_ID}`,
    staleTime: UPDATE_CHECK_TTL_MS,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
    retry: false,
  });

  // The query is disabled outside the deep link, and a disabled observer is
  // served cached data without ever refetching it. Age is checked here so a
  // verdict from hours ago isn't presented as the current one.
  const result =
    cachedResult && Date.now() - dataUpdatedAt < UPDATE_CHECK_TTL_MS ? cachedResult : undefined;

  useEffect(() => {
    alive.current = true;
    getCurrentVersion()
      .then((v) => {
        if (!alive.current) return;
        setVersion(v);
        setVersionResolved(true);
      })
      .catch(() => {
        if (alive.current) setVersionResolved(true);
      });
    getLinuxPackageKind()
      .then((k) => {
        if (!alive.current) return;
        setPackageKind(k);
        setPackageKindResolved(true);
      })
      .catch(() => {
        if (alive.current) setPackageKindResolved(true);
      });
    return () => {
      alive.current = false;
    };
  }, []);

  function handleCheck() {
    setInstallError(null);
    void refetch();
  }

  async function handleInstall() {
    setInstalling(true);
    setInstallError(null);
    try {
      await installUpdate(packageKind);
      // installUpdate relaunches the app on success; nothing left to do here.
    } catch (e) {
      if (alive.current) setInstallError(errText(e, "Install failed."));
    } finally {
      if (alive.current) setInstalling(false);
    }
  }

  return (
    <div>
      <SettingGroupLabel>About</SettingGroupLabel>
      <SettingGroup>
        <SettingRow
          label="linXiv"
          description={
            !versionResolved
              ? "Checking version…"
              : version
              ? `Version ${version}`
              : "Development build"
          }
        >
          <Button variant="muted" size="sm" onClick={handleCheck} disabled={checking}>
            {checking ? (
              <>
                <Spinner size={14} /> Checking…
              </>
            ) : (
              "Check for updates"
            )}
          </Button>
        </SettingRow>
        <SettingRow
          label="Check automatically"
          description="Look for a new release in the background on this schedule. Off by default."
          descriptionId="update-frequency-desc"
        >
          <AutoCheckControl />
        </SettingRow>
        {(checking || result || checkError) && (
          <SettingRow label="Update status">
            {checking ? (
              // Shown while a check is in flight so arriving from the banner's
              // Install link never lands on an empty row, and so a re-check
              // doesn't leave the previous verdict on screen.
              <span className="flex items-center gap-2 text-sm text-muted">
                <Spinner size={14} /> Checking…
              </span>
            ) : result ? (
              // A known result outranks a failed re-check: React Query keeps
              // `data` alongside `error`, and dropping it would take the
              // Install button away from someone who just followed the banner.
              <>
                <UpdateMessage
                  result={result}
                  packageKind={packageKind}
                  packageKindResolved={packageKindResolved}
                  onInstall={handleInstall}
                  installing={installing}
                  installError={installError}
                />
                {checkError && (
                  <span className="text-xs text-danger">
                    Last check failed: {checkError.message}
                  </span>
                )}
              </>
            ) : (
              checkError && (
                <span style={{ color: "var(--color-danger)" }}>{checkError.message}</span>
              )
            )}
          </SettingRow>
        )}
      </SettingGroup>
    </div>
  );
}
