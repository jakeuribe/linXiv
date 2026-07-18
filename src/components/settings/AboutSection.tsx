import { useEffect, useRef, useState } from "react";
import { isTauri } from "../../api/client";
import {
  checkForUpdates,
  getCurrentVersion,
  getLinuxPackageKind,
  installUpdate,
  openReleaseUrl,
  type UpdateResult,
} from "../../api/updates";
import { Button } from "../ui/button";
import { Spinner } from "../ui/spinner";
import { SettingGroup, SettingGroupLabel, SettingRow } from "./SettingRow";

function UpdateMessage({
  result,
  packageKind,
  onInstall,
  installing,
  installError,
}: {
  result: UpdateResult;
  packageKind: "deb" | "rpm" | null;
  onInstall: () => void;
  installing: boolean;
  installError: string | null;
}) {
  if (result.error) {
    return <span style={{ color: "var(--color-danger)" }}>{result.error}</span>;
  }
  if (result.hasUpdate && result.latest) {
    return (
      <span className="flex items-center gap-3 flex-wrap">
        <span style={{ color: "var(--color-success)" }}>
          Version {result.latest} is available.
        </span>
        {isTauri && (
          <Button variant="primary" size="sm" onClick={onInstall} disabled={installing}>
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
  const [version, setVersion] = useState<string | null>(null);
  const [versionResolved, setVersionResolved] = useState(false);
  const [checking, setChecking] = useState(false);
  const [result, setResult] = useState<UpdateResult | null>(null);
  const [packageKind, setPackageKind] = useState<"deb" | "rpm" | null>(null);
  const [installing, setInstalling] = useState(false);
  const [installError, setInstallError] = useState<string | null>(null);
  const alive = useRef(true);

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
    getLinuxPackageKind().then((k) => {
      if (alive.current) setPackageKind(k);
    });
    return () => {
      alive.current = false;
    };
  }, []);

  async function handleCheck() {
    setChecking(true);
    setResult(null);
    setInstallError(null);
    try {
      const r = await checkForUpdates();
      if (alive.current) setResult(r);
    } finally {
      if (alive.current) setChecking(false);
    }
  }

  async function handleInstall() {
    setInstalling(true);
    setInstallError(null);
    try {
      await installUpdate(packageKind);
      // installUpdate relaunches the app on success; nothing left to do here.
    } catch (e) {
      if (alive.current) setInstallError(e instanceof Error ? e.message : "Install failed.");
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
        {result && (
          <SettingRow label="Update status">
            <UpdateMessage
              result={result}
              packageKind={packageKind}
              onInstall={handleInstall}
              installing={installing}
              installError={installError}
            />
          </SettingRow>
        )}
      </SettingGroup>
    </div>
  );
}
