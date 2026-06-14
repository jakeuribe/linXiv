// Editor-plugin management surface (plan §4.4, ADR 0017 §Lifecycle): installed
// version + on-disk size, Check for updates (compat-gated against the release
// feed), Update (non-destructive re-install), and Uninstall (reclaim space).
// Mirrors AboutSection's button-initiated check + inline-message idiom so the
// two update surfaces can later merge into one global "Check for updates"
// (ADR 0017 "unified ... entry point").

import { useCallback, useEffect, useRef, useState } from "react";
import { isTauri } from "../../api/client";
import {
  checkUpdates,
  install,
  onInstallProgress,
  status,
  uninstall,
  type InstallProgress,
  type PluginStatus,
  type UpdateCheck,
} from "../../api/editorPlugin";
import { fmtMB, errMessage } from "../../lib/editorPluginUtils";
import { Button } from "../ui/button";
import { Dialog } from "../ui/dialog";
import { Spinner } from "../ui/spinner";
import { Section } from "./Section";

function CheckMessage({ check }: { check: UpdateCheck }) {
  if (check.noCompatibleRelease) {
    return (
      <span style={{ color: "var(--color-danger)" }}>
        No editor release matches this version of linXiv — an app update may be
        required.
      </span>
    );
  }
  if (check.updateAvailable && check.latestVersion) {
    return (
      <span style={{ color: "var(--color-success)" }}>
        Version {check.latestVersion} is available
        {check.downloadBytes != null ? ` (${fmtMB(check.downloadBytes)} download)` : ""}.
      </span>
    );
  }
  return <span style={{ color: "var(--color-success)" }}>The editor is up to date.</span>;
}

export function EditorPluginSection({ defaultOpen = false }: { defaultOpen?: boolean } = {}) {
  const [info, setInfo] = useState<PluginStatus | null>(null);
  const [check, setCheck] = useState<UpdateCheck | null>(null);
  const [busy, setBusy] = useState<"check" | "install" | "uninstall" | null>(null);
  const [progress, setProgress] = useState<InstallProgress | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Uninstall confirmation runs through the in-app Dialog below — window.confirm
  // is suppressed in some webviews (e.g. Linux WebKitGTK returns false without
  // showing anything), which would make Uninstall do nothing.
  const [confirmUninstall, setConfirmUninstall] = useState(false);
  // Guards against committing state from in-flight async work after unmount.
  const alive = useRef(true);

  // `isStale` is per effect invocation: a refresh started before a StrictMode
  // teardown stays cancelled even after the re-run sets `alive` back to true,
  // so it can't commit over a newer run's result.
  const refresh = useCallback(async (isStale: () => boolean) => {
    try {
      const s = await status();
      if (!isStale()) setInfo(s);
    } catch (e) {
      if (!isStale()) setError(errMessage(e));
    }
  }, []);

  useEffect(() => {
    alive.current = true;
    let stale = false;
    if (isTauri) void refresh(() => stale);
    return () => {
      stale = true;
      alive.current = false;
    };
  }, [refresh]);

  async function handleCheck() {
    setBusy("check");
    setError(null);
    setCheck(null);
    try {
      const c = await checkUpdates();
      if (alive.current) setCheck(c);
    } catch (e) {
      if (alive.current) setError(errMessage(e));
    } finally {
      if (alive.current) setBusy(null);
    }
  }

  // Install and Update are the same non-destructive pipeline (ADR 0017): the
  // current install keeps serving until the replacement fully verifies.
  async function handleInstall() {
    setBusy("install");
    setError(null);
    setProgress(null);
    const unlisten = await onInstallProgress((p) => {
      if (alive.current) setProgress(p);
    }).catch(() => null);
    try {
      const s = await install();
      if (alive.current) {
        setInfo(s);
        setCheck(null);
      }
    } catch (e) {
      if (alive.current) setError(errMessage(e));
    } finally {
      unlisten?.();
      if (alive.current) {
        setBusy(null);
        setProgress(null);
      }
    }
  }

  async function handleUninstall() {
    setBusy("uninstall");
    setError(null);
    try {
      const s = await uninstall();
      if (alive.current) {
        setInfo(s);
        setCheck(null);
      }
    } catch (e) {
      if (alive.current) setError(errMessage(e));
    } finally {
      if (alive.current) setBusy(null);
    }
  }

  if (!isTauri) {
    return (
      <Section title="LaTeX editor" defaultOpen={defaultOpen}>
        <p className="text-sm text-muted">
          The editor plugin is managed from the desktop app (in browser dev the
          editor runs from its own dev server).
        </p>
      </Section>
    );
  }

  const installing = busy === "install";
  const pct =
    progress && progress.total > 0
      ? Math.min(100, Math.round((progress.received / progress.total) * 100))
      : null;

  return (
    <Section title="LaTeX editor" defaultOpen={defaultOpen}>
      <div className="flex items-center justify-between gap-4 flex-wrap">
        <div>
          <p className="text-sm font-medium text-text">Editor plugin</p>
          <p className="text-xs text-muted mt-0.5">
            {info == null ? (
              "Reading status…"
            ) : info.installed ? (
              <>
                Version {info.pluginVersion ?? "?"} installed · {fmtMB(info.onDiskBytes)} on disk
              </>
            ) : (
              "Not installed — the Editor tab offers the download."
            )}
          </p>
        </div>
        <div className="flex items-center gap-2 flex-wrap">
          <Button variant="muted" size="sm" onClick={() => void handleCheck()} disabled={busy != null}>
            {busy === "check" ? <Spinner size={14} /> : "Check for updates"}
          </Button>
          {check?.updateAvailable && !check.noCompatibleRelease && (
            <Button variant="primary" size="sm" onClick={() => void handleInstall()} disabled={busy != null}>
              {installing ? <Spinner size={14} /> : info?.installed ? "Update" : "Install"}
            </Button>
          )}
          {info?.installed && (
            <Button variant="muted" size="sm" onClick={() => setConfirmUninstall(true)} disabled={busy != null}>
              {busy === "uninstall" ? <Spinner size={14} /> : "Uninstall"}
            </Button>
          )}
        </div>
      </div>
      {(check || error || installing) && (
        <div className="mt-3 text-sm">
          {error ? (
            <span style={{ color: "var(--color-danger)" }}>{error}</span>
          ) : installing ? (
            <span className="text-muted">
              {progress?.phase === "promote"
                ? "Finishing up…"
                : progress?.phase === "verify"
                  ? "Verifying download…"
                  : `Downloading${pct != null ? ` ${pct}%` : "…"}`}
            </span>
          ) : check ? (
            <CheckMessage check={check} />
          ) : null}
        </div>
      )}
      <Dialog
        open={confirmUninstall}
        onClose={() => setConfirmUninstall(false)}
        title="Uninstall editor"
      >
        <p className="text-sm text-muted mb-4">
          Uninstall the LaTeX editor and reclaim its disk space?
        </p>
        <div className="flex justify-end gap-2">
          <Button variant="ghost" size="sm" onClick={() => setConfirmUninstall(false)}>
            Cancel
          </Button>
          <Button
            variant="danger"
            size="sm"
            onClick={() => {
              setConfirmUninstall(false);
              void handleUninstall();
            }}
          >
            Uninstall
          </Button>
        </div>
      </Dialog>
    </Section>
  );
}
