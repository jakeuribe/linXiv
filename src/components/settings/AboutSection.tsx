import { useEffect, useRef, useState } from "react";
import { checkForUpdates, getCurrentVersion, openReleaseUrl, type UpdateResult } from "../../api/updates";
import { Button } from "../ui/button";
import { Spinner } from "../ui/spinner";
import { Section } from "./Section";

function UpdateMessage({ result }: { result: UpdateResult }) {
  if (result.error) {
    return <span style={{ color: "var(--color-danger)" }}>{result.error}</span>;
  }
  if (result.hasUpdate && result.latest) {
    return (
      <span className="flex items-center gap-3 flex-wrap">
        <span style={{ color: "var(--color-success)" }}>
          Version {result.latest} is available.
        </span>
        <Button
          variant="primary"
          size="sm"
          onClick={() => openReleaseUrl(result.releaseUrl).catch(console.error)}
        >
          Download
        </Button>
      </span>
    );
  }
  if (result.latest === null) {
    return <span className="text-muted">No published releases yet.</span>;
  }
  // The latest release exists but we couldn't read the installed version
  // (browser dev build) — show it for reference without claiming up-to-date.
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

export function AboutSection({ defaultOpen = true }: { defaultOpen?: boolean } = {}) {
  const [version, setVersion] = useState<string | null>(null);
  const [versionResolved, setVersionResolved] = useState(false);
  const [checking, setChecking] = useState(false);
  const [result, setResult] = useState<UpdateResult | null>(null);
  // Guards against committing state from in-flight async work after unmount.
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
    return () => {
      alive.current = false;
    };
  }, []);

  async function handleCheck() {
    setChecking(true);
    setResult(null);
    try {
      const r = await checkForUpdates();
      if (alive.current) setResult(r);
    } finally {
      if (alive.current) setChecking(false);
    }
  }

  return (
    <Section title="About" defaultOpen={defaultOpen}>
      <div className="flex items-center justify-between gap-4 flex-wrap">
        <div>
          <p className="text-sm font-medium text-text">linXiv</p>
          <p className="text-xs text-muted mt-0.5">
            {!versionResolved
              ? "Checking version…"
              : version
              ? `Version ${version}`
              : "Development build"}
          </p>
        </div>
        <Button variant="muted" size="sm" onClick={handleCheck} disabled={checking}>
          {checking ? (
            <>
              <Spinner size={14} /> Checking…
            </>
          ) : (
            "Check for updates"
          )}
        </Button>
      </div>

      {result && (
        <div className="mt-3 text-sm">
          <UpdateMessage result={result} />
        </div>
      )}
    </Section>
  );
}
