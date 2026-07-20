// EditorPage.tsx
// -----------------------------------------------------------------------------
// Host-side full-canvas view that embeds the TeXbrain editor in an <iframe> and
// drives it over the EditorBridge. Modeled on GraphPage.tsx (full-height flex:
// header bar + flex-1 iframe), it wires an EditorBridgeClient to the iframe's
// contentWindow, pushes live linXiv theme updates to the guest, and now mounts a
// real LaTeX project: the header lists editor projects (frontmatter-flagged notes,
// see service/editor_project.py), and selecting one pushes its DocOpenPayload via
// texbrain:doc:open. Reads/writes flow through ApiFsResponder → the on-disk vault,
// so edits persist.
// -----------------------------------------------------------------------------

import { useCallback, useEffect, useRef, useState } from "react";
import { useLocation } from "react-router-dom";
import { useThemeStore, registerEditorFrame } from "../stores/theme";
import {
  EditorBridgeClient,
  pushThemeToEditor,
  EDITOR_ORIGIN,
  EDITOR_SRC,
  type ThemePushState,
} from "./editorConfig";
import { ApiFsResponder } from "../lib/editorFsResponder";
import { HostFsRouter } from "../lib/editorDiskFs";
import { pickFolder, isSupportedBridgeProtocol } from "../api/editorPlugin";
import { isTauri } from "../api/client";
import {
  listEditorProjects,
  createEditorProject,
  getEditorDoc,
  type EditorProjectSummary,
} from "../api/editor";
import {
  install as installPlugin,
  checkUpdates as checkPluginUpdates,
  status as pluginStatus,
  onInstallProgress,
  type InstallProgress,
  type UpdateCheck,
} from "../api/editorPlugin";
import { fmtMB, errMessage } from "../lib/editorPluginUtils";
import { Dialog } from "../components/ui/dialog";
import { formSubmitOnCtrlEnter } from "../lib/submitShortcut";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";

// ---- Editor-plugin install gate (plan §4.3, ADR 0017 §Lifecycle) ------------
// In DEV the editor comes from its own dev server — no plugin, no gate. In PROD
// the editor is the downloaded Editor plugin served via texbrain://; AppShell's
// keep-alive pattern mounts this page (and would mount the iframe) at app BOOT,
// so the iframe src must not be set until status() reports installed — render
// the "Install the LaTeX editor" card in its place instead.

type PluginGate =
  | { state: "checking" }
  | { state: "ready" }
  // A PROD frontend running in a plain browser (e.g. `vite preview` of the
  // built dist): there is no Tauri runtime, so the plugin commands and the
  // texbrain:// scheme don't exist — show a friendly notice, never invoke.
  | { state: "browser" }
  | {
      state: "missing";
      check: UpdateCheck | null;
      error: string | null;
      installing: boolean;
      progress: InstallProgress | null;
    };

function InstallCard({
  gate,
  onInstall,
}: {
  gate: Extract<PluginGate, { state: "missing" }>;
  onInstall: () => void;
}) {
  const { check, error, installing, progress } = gate;
  const artifactLabel =
    progress?.artifact === "texliveCache" ? "TeX Live" : "editor";
  const pct =
    progress && progress.total > 0
      ? Math.min(100, Math.round((progress.received / progress.total) * 100))
      : null;

  return (
    <div className="flex-1 flex items-center justify-center">
      <div className="max-w-md w-full border border-border rounded-lg p-6 bg-panel">
        <h2 className="text-base font-semibold text-text">Install the LaTeX editor</h2>
        {check?.noCompatibleRelease ? (
          <p className="text-sm text-muted mt-2">
            No editor release matches this version of linXiv; check the recent
            releases for more information.
          </p>
        ) : (
          <p className="text-sm text-muted mt-2">
            The editor is a one-time download (about {fmtMB(check?.downloadBytes)};
            ~78 MB of TeX Live on disk). After installing, it works fully
            offline.
          </p>
        )}
        {error && <p className="text-sm text-danger mt-3">{error}</p>}
        {installing ? (
          <div className="mt-4">
            <p className="text-sm text-text">
              {progress?.phase === "promote"
                ? "Finishing up…"
                : progress?.phase === "verify"
                  ? `Verifying the ${artifactLabel} download…`
                  : `Downloading the ${artifactLabel}… ${pct != null ? `${pct}%` : ""}`}
            </p>
            <div className="mt-2 h-2 rounded bg-bg overflow-hidden border border-border">
              <div
                className="h-full bg-accent transition-all"
                style={{ width: `${pct ?? 0}%` }}
              />
            </div>
          </div>
        ) : (
          !check?.noCompatibleRelease && (
            <button
              type="button"
              className="mt-4 text-sm bg-accent text-bg rounded px-4 py-2 hover:opacity-90"
              onClick={onInstall}
            >
              {error ? "Retry install" : "Install"}
            </button>
          )
        )}
      </div>
    </div>
  );
}

export default function EditorPage() {
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const bridgeRef = useRef<EditorBridgeClient | null>(null);

  // The active project's note id, read lazily by the long-lived bridge/FS responder
  // and onReady handler so switching projects never needs the bridge rebuilt.
  const noteIdRef = useRef<number | null>(null);
  const guestReadyRef = useRef(false);
  // A project chosen before the guest finished its handshake; mounted on ready.
  const pendingOpenRef = useRef<number | null>(null);
  // The guest's unsaved-edits flag (from texbrain:dirty); guards project switches.
  const dirtyRef = useRef(false);
  // Absolute path of a host-picked disk folder the guest opened IN PLACE of a
  // vault project (its "Open Folder" → texbrain:pick:folder). While set, the
  // HostFsRouter routes every fs op here instead of the vault; cleared whenever
  // a vault project mounts (mountProject).
  const diskRootRef = useRef<string | null>(null);

  const [projects, setProjects] = useState<EditorProjectSummary[]>([]);
  const [currentNoteId, setCurrentNoteId] = useState<number | null>(null);
  // Basename of the picked disk folder, for the project <select> placeholder
  // (the dropdown is vault-project-only; a disk project deselects it).
  const [diskName, setDiskName] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Non-fatal: the live guest reported a bridge protocol this host doesn't
  // support (plan Phase 5; the real guard is the install-time manifest gate).
  const [protocolWarning, setProtocolWarning] = useState<string | null>(null);
  // noteId awaiting the unsaved-changes confirmation dialog (null = closed).
  const [pendingSwitch, setPendingSwitch] = useState<number | null>(null);
  // Draft name in the new-project dialog (null = dialog closed).
  const [newProjectName, setNewProjectName] = useState<string | null>(null);

  // DEV skips the install gate entirely (ADR 0017: "in development the plugin
  // is not downloaded" — the editor dev server serves the iframe).
  const [gate, setGate] = useState<PluginGate>(
    import.meta.env.DEV ? { state: "ready" } : { state: "checking" }
  );
  const pluginReady = gate.state === "ready";

  // PROD boot: resolve installed-or-not before ever setting the iframe src.
  // status() is LOCAL (reads the on-disk cache) — no network. We deliberately do
  // NOT call checkUpdates() here: AppShell keep-alive mounts this page at app
  // launch regardless of route, and ADR 0017 forbids unsolicited background
  // network (a per-launch unauthenticated GitHub hit for users who never open
  // the editor). The compat-gated download size is fetched lazily once the user
  // actually views the Editor tab (the effect below).
  useEffect(() => {
    if (import.meta.env.DEV) return;
    // PROD frontend outside Tauri (browser preview of the built dist): no
    // runtime, no plugin commands — don't invoke (it throws
    // "__TAURI_INTERNALS__ is undefined"), just show the notice.
    if (!isTauri) {
      setGate({ state: "browser" });
      return;
    }
    let alive = true;
    void (async () => {
      try {
        const s = await pluginStatus();
        if (!alive) return;
        setGate(
          s.installed
            ? { state: "ready" }
            : { state: "missing", check: null, error: null, installing: false, progress: null }
        );
      } catch (e) {
        if (alive) {
          setGate({
            state: "missing",
            check: null,
            error: errMessage(e),
            installing: false,
            progress: null,
          });
        }
      }
    })();
    return () => {
      alive = false;
    };
  }, []);

  // Lazily fill the install card's download-size copy the first time the user is
  // actually looking at the Editor tab and the plugin is missing — the only
  // point a network update-check is user-initiated rather than at boot.
  const onEditorRoute = useLocation().pathname === "/editor";
  const sizeFetchedRef = useRef(false);
  // Primitive projections of the union so the effect deps don't reference
  // variant-only fields (TS can't narrow gate.check/gate.installing in a deps
  // array) and the effect re-runs only on the transitions it cares about.
  const gateMissingNoCheck = gate.state === "missing" && !gate.check;
  const gateInstalling = gate.state === "missing" && gate.installing;
  useEffect(() => {
    if (import.meta.env.DEV) return;
    if (!onEditorRoute || sizeFetchedRef.current) return;
    if (!gateMissingNoCheck || gateInstalling) return;
    sizeFetchedRef.current = true;
    let alive = true;
    void (async () => {
      try {
        const check = await checkPluginUpdates();
        // If the user navigated away mid-flight, clear the guard so the size is
        // re-fetched next time they view the tab — otherwise the card would be
        // stuck on "? MB" for the rest of the session despite a successful call.
        if (alive) setGate((g) => (g.state === "missing" ? { ...g, check } : g));
        else sizeFetchedRef.current = false;
      } catch {
        sizeFetchedRef.current = false; // offline — allow a later retry; card shows "? MB"
      }
    })();
    return () => {
      alive = false;
    };
    // Depend on the primitive projections, NOT the whole gate object: every
    // install-progress event makes a new gate reference and would otherwise
    // tear down/re-run this effect ~50× per install.
  }, [onEditorRoute, gateMissingNoCheck, gateInstalling]);

  // Re-resolve the gate every time the user (re)enters the Editor tab:
  // Settings' Uninstall (EditorPluginSection) doesn't notify this page, so a
  // gate stuck at 'ready' would keep rendering the iframe against a
  // texbrain:// scheme that now 404s — a blank editor with no recovery path.
  // status() is LOCAL (on-disk cache read), so this costs no network. The
  // reconcile only flips ready↔missing when not mid-install, leaving the
  // install card's check/error/progress sub-state alone otherwise.
  useEffect(() => {
    if (import.meta.env.DEV || !isTauri || !onEditorRoute) return;
    let alive = true;
    void pluginStatus()
      .then((s) => {
        if (!alive) return;
        setGate((g) => {
          if (s.installed)
            return g.state === "missing" && !g.installing ? { state: "ready" } : g;
          return g.state === "ready"
            ? { state: "missing", check: null, error: null, installing: false, progress: null }
            : g;
        });
      })
      .catch(() => {}); // boot effect surfaces status() errors; this is best-effort
    return () => {
      alive = false;
    };
  }, [onEditorRoute]);

  // Guards handleInstall's post-await setGate calls (progress events, results)
  // against an unmounted component. AppShell's keep-alive means this page never
  // unmounts today, but the install flow shouldn't depend on that (mirrors
  // EditorPluginSection's alive ref).
  const aliveRef = useRef(true);
  useEffect(() => {
    aliveRef.current = true; // StrictMode remount: reset after the teardown below
    return () => {
      aliveRef.current = false;
    };
  }, []);

  const handleInstall = useCallback(async () => {
    setGate((g) =>
      g.state === "missing" ? { ...g, installing: true, error: null, progress: null } : g
    );
    const unlisten = await onInstallProgress((p) => {
      if (aliveRef.current) setGate((g) => (g.state === "missing" ? { ...g, progress: p } : g));
    }).catch(() => null);
    try {
      const s = await installPlugin();
      if (s.installed && aliveRef.current) setGate({ state: "ready" });
    } catch (e) {
      if (aliveRef.current)
        setGate((g) => (g.state === "missing" ? { ...g, error: errMessage(e) } : g));
    } finally {
      unlisten?.();
      // Always drop the installing flag (the catch above relies on this too):
      // install() resolving with installed:false would otherwise wedge the card
      // with a progress bar and no retry button.
      if (aliveRef.current)
        setGate((g) =>
          g.state === "missing" ? { ...g, installing: false, progress: null } : g
        );
    }
  }, []);

  // Live theme delivery is owned by the theme store itself (stores/theme.ts
  // applyAndSet → pushThemeToEditor on the registered frame), so EditorPage no
  // longer subscribes to palette slices or re-renders on theme change. The two
  // spots that still need the palette directly (the onReady handshake reply and
  // the onLoad nudge) read it fresh via useThemeStore.getState().

  // Fetch a project's doc + push it to the guest. Sets noteIdRef BEFORE doc:open so
  // the FS ops the guest fires while mounting target the right vault.
  const mountProject = useCallback(async (noteId: number) => {
    try {
      const doc = await getEditorDoc(noteId);
      noteIdRef.current = noteId;
      // A vault project replaces any picked disk folder: route fs ops back to
      // the vault BEFORE doc:open so the guest's mount reads the right tree.
      diskRootRef.current = null;
      setDiskName(null);
      setCurrentNoteId(noteId);
      setError(null);
      // Stamp the stable project identity (noteId) onto the payload so the
      // editor's idempotency guard distinguishes a real switch from a re-send.
      bridgeRef.current?.sendDocOpen({ ...doc, projectId: noteId });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);
  // Keep a ref so the bridge's once-constructed onReady handler calls the latest.
  const mountRef = useRef(mountProject);
  mountRef.current = mountProject;

  // Perform the switch (no dirty guard): mount now if the guest is ready, else
  // remember it for onReady.
  const commitOpenProject = useCallback((noteId: number) => {
    if (guestReadyRef.current && bridgeRef.current) {
      void mountProject(noteId);
    } else {
      pendingOpenRef.current = noteId;
      noteIdRef.current = noteId;
      setCurrentNoteId(noteId);
    }
  }, [mountProject]);

  // Open a project. Switching away from a project with unsaved edits would
  // discard them (the guest re-mounts on doc:open), so confirm first via the
  // in-app dialog — window.confirm is suppressed in some webviews (e.g. Linux
  // WebKitGTK returns false without showing anything), which would silently
  // block every switch. Opening the dialog re-renders, snapping the controlled
  // <select> back to currentNoteId until the user confirms.
  const openProject = useCallback((noteId: number) => {
    if (noteId === noteIdRef.current) return; // already the open project
    // Guard unsaved edits in the open VAULT project or a host-picked DISK
    // folder — switching re-mounts the guest and would discard them either way.
    if (dirtyRef.current && (noteIdRef.current != null || diskRootRef.current != null)) {
      setPendingSwitch(noteId);
      return;
    }
    commitOpenProject(noteId);
  }, [commitOpenProject]);

  const refreshProjects = useCallback(async () => {
    try {
      const ps = await listEditorProjects();
      setProjects(ps);
      return ps;
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      return [];
    }
  }, []);

  // The project name is collected via the in-app dialog below — window.prompt
  // is suppressed in some webviews (e.g. Linux WebKitGTK returns null without
  // showing anything), which would make "New project" do nothing.
  const submitNewProject = useCallback(async () => {
    const name = newProjectName?.trim();
    if (!name) return;
    setNewProjectName(null); // close the dialog
    try {
      const created = await createEditorProject({ project_name: name });
      await refreshProjects();
      openProject(created.noteId);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [newProjectName, refreshProjects, openProject]);

  // Create the bridge once the iframe element exists; tear it down on unmount.
  // Gated on pluginReady: while the install card shows there IS no iframe — the
  // effect re-runs when the gate opens and binds the freshly-mounted frame.
  useEffect(() => {
    if (!pluginReady) return;
    // Guards async bridge callbacks (the native folder picker can outlive this
    // effect cycle) so a stale closure doesn't mutate refs/state after destroy.
    let effectAlive = true;
    const frame = iframeRef.current?.contentWindow ?? null;
    registerEditorFrame(frame);
    const bridge = new EditorBridgeClient(frame, EDITOR_ORIGIN, {
      // The client replies to the guest's 'texbrain:ready' handshake by pushing
      // this resolved theme before first paint (no Amber flash). Reading from
      // the store via getState() avoids a stale closure in the long-lived bridge.
      getThemeState: (): ThemePushState => {
        const s = useThemeStore.getState();
        return {
          preset: s.preset,
          mode: s.mode,
          overrides: s.overrides,
          overrideAlphas: s.overrideAlphas,
        };
      },
      // Real persistence: every guest FS op is forwarded to the on-disk vault for
      // whatever project is currently open (noteIdRef) — unless the guest opened
      // a host-picked disk folder (diskRootRef), which the router serves via the
      // Tauri fs plugin instead.
      fs: new HostFsRouter(
        () => diskRootRef.current,
        new ApiFsResponder(() => noteIdRef.current)
      ),
      // The guest's "Open Folder": show the native directory picker, re-root the
      // fs router at the pick, and deselect the vault-project dropdown. Resolving
      // null = user cancelled (the guest keeps its current project mounted).
      // pickFolder is a plugin command (tauri-plugin-texbrain) that shows the
      // native directory dialog and extends the fs plugin's scope recursively
      // for the picked folder (ADR 0018). It's in the plugin, not the host,
      // so the plugin stays self-contained.
      onPickFolder: async () => {
        const dir = await pickFolder();
        // This bridge was torn down while the picker was open (e.g. StrictMode
        // re-run): drop the result instead of desyncing the replacement bridge.
        if (!effectAlive || !dir) return null;
        diskRootRef.current = dir;
        noteIdRef.current = null;
        setCurrentNoteId(null);
        const name = dir.split(/[/\\]/).filter(Boolean).pop() ?? dir;
        setDiskName(name);
        return name;
      },
      // The guest (re)booted: mount the selected/pending project. Also fires on a
      // hot-reload, so the open project re-mounts after an iframe reload. (The guest
      // ignores a same-project re-send, so this won't clobber unsaved buffers.)
      onReady: (protocol?: number) => {
        guestReadyRef.current = true;
        dirtyRef.current = false; // fresh guest starts clean
        setProtocolWarning(
          protocol != null && !isSupportedBridgeProtocol(protocol)
            ? `The editor reports bridge protocol ${protocol}, which this version of linXiv doesn't support; if features misbehave, check the linXiv GitHub for more information.`
            : null
        );
        const pid = pendingOpenRef.current ?? noteIdRef.current;
        pendingOpenRef.current = null;
        if (pid != null) void mountRef.current(pid);
      },
      // Track the guest's unsaved-edits flag so openProject can guard switches.
      onDirty: (dirty: boolean) => {
        dirtyRef.current = dirty;
      },
    });
    bridgeRef.current = bridge;
    return () => {
      effectAlive = false;
      bridge.destroy();
      bridgeRef.current = null;
      guestReadyRef.current = false;
      registerEditorFrame(null);
    };
  }, [pluginReady]);

  // Load the project list and auto-open the most recent one the first time the
  // user actually views the Editor tab — NOT at mount: AppShell's keep-alive
  // mounts this page at app BOOT, so an unconditional fetch would hit the
  // backend on every launch, and a failure would set the error banner on a
  // hidden (display:none) component where nobody sees it.
  const projectsFetchedRef = useRef(false);
  useEffect(() => {
    if (!onEditorRoute || projectsFetchedRef.current) return;
    projectsFetchedRef.current = true;
    void refreshProjects().then((ps) => {
      if (ps.length && noteIdRef.current == null) openProject(ps[0].noteId);
    });
  }, [onEditorRoute, refreshProjects, openProject]);

  // NOTE: live theme changes are pushed to the editor by the theme store itself
  // (stores/theme.ts applyAndSet → pushThemeToEditor on the registered frame),
  // which also covers programmatic theme callers outside React. We deliberately
  // do NOT duplicate that here — a second push would send the guest an identical
  // texbrain:theme on every change. Initial theming is the onReady handshake
  // reply + the onLoad nudge below.

  return (
    <div className="w-full h-full flex flex-col">
      <div className="p-4 border-b border-border flex items-center gap-3">
        <h1 className="text-lg font-semibold text-text">Editor</h1>
        <span className="text-sm text-muted">LaTeX editor (TeXbrain)</span>
        {/* Project management is only meaningful once the editor is mounted;
            while the install card shows (prod, not-installed) it stays hidden. */}
        {pluginReady && (
        <div className="flex items-center gap-2 ml-auto">
          {projects.length > 0 && (
            <select
              className="text-sm bg-panel text-text border border-border rounded px-2 py-1"
              value={currentNoteId ?? ""}
              onChange={(e) => {
                // Number("") coerces to 0, not NaN — ignore the disk-folder
                // placeholder (value="") instead of calling openProject(0).
                const id = Number(e.target.value);
                if (Number.isInteger(id) && id > 0) openProject(id);
              }}
              title="Open editor project"
            >
              {/* Disk-folder placeholder: a host-picked folder isn't a vault
                  project, so it deselects the dropdown — show its name instead
                  of silently displaying the first project. */}
              {diskName != null && (
                <option value="" disabled hidden>
                  {diskName} (folder)
                </option>
              )}
              {projects.map((p) => (
                <option key={p.noteId} value={p.noteId}>
                  {p.projectName}
                </option>
              ))}
            </select>
          )}
          <button
            type="button"
            className="text-sm bg-accent text-bg rounded px-3 py-1 hover:opacity-90"
            onClick={() => setNewProjectName("Untitled")}
          >
            New project
          </button>
        </div>
        )}
      </div>
      {/* In-app replacements for window.prompt/confirm (suppressed in some
          webviews, e.g. Linux WebKitGTK) — same Dialog pattern as TrashSection. */}
      <Dialog
        open={newProjectName != null}
        onClose={() => setNewProjectName(null)}
        title="New project"
      >
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void submitNewProject();
          }}
          onKeyDown={formSubmitOnCtrlEnter}
        >
          <Input
            autoFocus
            value={newProjectName ?? ""}
            onChange={(e) => setNewProjectName(e.target.value)}
            placeholder="Project name"
            aria-label="Project name"
          />
          <div className="flex justify-end gap-2 mt-4">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={() => setNewProjectName(null)}
            >
              Cancel
            </Button>
            <Button
              type="submit"
              variant="primary"
              size="sm"
              disabled={!newProjectName?.trim()}
            >
              Create
            </Button>
          </div>
        </form>
      </Dialog>
      <Dialog
        open={pendingSwitch != null}
        onClose={() => setPendingSwitch(null)}
        title="Unsaved changes"
      >
        <p className="text-sm text-muted mb-4">
          This project has unsaved changes that will be lost. Switch project
          anyway?
        </p>
        <div className="flex justify-end gap-2">
          <Button variant="ghost" size="sm" onClick={() => setPendingSwitch(null)}>
            Cancel
          </Button>
          <Button
            variant="danger"
            size="sm"
            onClick={() => {
              const id = pendingSwitch;
              setPendingSwitch(null);
              if (id != null) commitOpenProject(id);
            }}
          >
            Switch anyway
          </Button>
        </div>
      </Dialog>
      {error && (
        <div className="px-4 py-2 text-sm text-danger border-b border-border">
          {error}
        </div>
      )}
      {protocolWarning && (
        <div className="px-4 py-2 text-sm text-muted border-b border-border">
          ⚠ {protocolWarning}
        </div>
      )}
      {gate.state === "browser" && (
        <div className="flex-1 flex items-center justify-center">
          <p className="max-w-md text-sm text-muted text-center">
            The LaTeX editor runs inside the linXiv desktop app for now,
            browser view isn&apos;t supported yet.
          </p>
        </div>
      )}
      {gate.state === "checking" && (
        <div className="flex-1 flex items-center justify-center text-sm text-muted">
          Checking the editor plugin…
        </div>
      )}
      {gate.state === "missing" && <InstallCard gate={gate} onInstall={() => void handleInstall()} />}
      {pluginReady && (
      <iframe
        ref={iframeRef}
        src={EDITOR_SRC}
        className="flex-1 border-0 w-full"
        title="TeXbrain LaTeX editor"
        // No `sandbox` attr: the editor needs full same-origin privileges (the
        // SwiftLaTeX worker fetches wasm with credentials:'same-origin', uses
        // IndexedDB, etc.), which a non-sandboxed iframe already has. (`allow=`
        // is Permissions-Policy and has no "same-origin" token — omitted.)
        // COOP/COEP isolation for the worker is tracked in the asset-serving-note.
        onLoad={() => {
          // The mount effect captured the pre-navigation (about:blank) window;
          // re-register the freshly-loaded guest so the store-side push targets
          // the real editor, then nudge a theme push in case the bridge missed
          // the 'texbrain:ready' handshake (e.g. a hot reload after mount).
          const frame = iframeRef.current?.contentWindow ?? null;
          registerEditorFrame(frame);
          // Read the palette fresh (getState), not from the render closure — a
          // theme change while the iframe was loading would otherwise nudge a
          // stale palette and leave the editor mis-themed until the next change.
          const s = useThemeStore.getState();
          pushThemeToEditor(frame, EDITOR_ORIGIN, {
            preset: s.preset,
            mode: s.mode,
            overrides: s.overrides,
            overrideAlphas: s.overrideAlphas,
          });
        }}
      />
      )}
    </div>
  );
}
