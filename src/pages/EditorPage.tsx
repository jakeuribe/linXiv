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

import { useCallback, useEffect, useReducer, useRef, useState } from "react";
import { useThemeStore, registerEditorFrame } from "../stores/theme";
import {
  EditorBridgeClient,
  pushThemeToEditor,
  EDITOR_ORIGIN,
  EDITOR_SRC,
  type ThemePushState,
} from "./editorConfig";
import { ApiFsResponder } from "../lib/editorFsResponder";
import {
  listEditorProjects,
  createEditorProject,
  getEditorDoc,
  type EditorProjectSummary,
} from "../api/editor";

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

  const [projects, setProjects] = useState<EditorProjectSummary[]>([]);
  const [currentNoteId, setCurrentNoteId] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Force a re-render so the controlled <select> snaps back to currentNoteId after a
  // declined switch (no state actually changed, but the DOM value did).
  const [, forceRerender] = useReducer((n: number) => n + 1, 0);

  // Subscribe to the theme store the same way GraphPage does, so live theme
  // edits re-theme the embedded editor. We read the four resolvable inputs and
  // push resolved colors over the wire (the guest never needs linXiv's PRESETS).
  const preset = useThemeStore((s) => s.preset);
  const mode = useThemeStore((s) => s.mode);
  const overrides = useThemeStore((s) => s.overrides);
  const overrideAlphas = useThemeStore((s) => s.overrideAlphas);

  // Fetch a project's doc + push it to the guest. Sets noteIdRef BEFORE doc:open so
  // the FS ops the guest fires while mounting target the right vault.
  const mountProject = useCallback(async (noteId: number) => {
    try {
      const doc = await getEditorDoc(noteId);
      noteIdRef.current = noteId;
      setCurrentNoteId(noteId);
      setError(null);
      bridgeRef.current?.sendDocOpen(doc);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);
  // Keep a ref so the bridge's once-constructed onReady handler calls the latest.
  const mountRef = useRef(mountProject);
  mountRef.current = mountProject;

  // Open a project: mount now if the guest is ready, else remember it for onReady.
  // Switching away from a project with unsaved edits would discard them (the guest
  // re-mounts on doc:open), so confirm first. The <select> is controlled by
  // currentNoteId, so a declined switch reverts the dropdown automatically.
  const openProject = useCallback((noteId: number) => {
    if (noteId === noteIdRef.current) return; // already the open project
    if (dirtyRef.current && noteIdRef.current != null) {
      const ok = window.confirm(
        "This project has unsaved changes that will be lost. Switch project anyway?"
      );
      if (!ok) {
        forceRerender(); // revert the controlled <select> to the still-open project
        return;
      }
    }
    if (guestReadyRef.current && bridgeRef.current) {
      void mountProject(noteId);
    } else {
      pendingOpenRef.current = noteId;
      noteIdRef.current = noteId;
      setCurrentNoteId(noteId);
    }
  }, [mountProject]);

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

  const handleNewProject = useCallback(async () => {
    const name = window.prompt("New project name:", "Untitled");
    if (!name) return;
    try {
      const created = await createEditorProject({ project_name: name });
      await refreshProjects();
      openProject(created.noteId);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [refreshProjects, openProject]);

  // Create the bridge once the iframe element exists; tear it down on unmount.
  useEffect(() => {
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
      // whatever project is currently open (noteIdRef).
      fs: new ApiFsResponder(() => noteIdRef.current),
      // The guest (re)booted: mount the selected/pending project. Also fires on a
      // hot-reload, so the open project re-mounts after an iframe reload. (The guest
      // ignores a same-project re-send, so this won't clobber unsaved buffers.)
      onReady: () => {
        guestReadyRef.current = true;
        dirtyRef.current = false; // fresh guest starts clean
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
      bridge.destroy();
      bridgeRef.current = null;
      guestReadyRef.current = false;
      registerEditorFrame(null);
    };
  }, []);

  // Load the project list on mount and auto-open the most recent one.
  useEffect(() => {
    void refreshProjects().then((ps) => {
      if (ps.length && noteIdRef.current == null) openProject(ps[0].noteId);
    });
  }, [refreshProjects, openProject]);

  // Push the current theme to the editor on every store change. The guest
  // re-themes live from these resolved colors (no preset name on the wire).
  useEffect(() => {
    const frame = iframeRef.current?.contentWindow ?? null;
    pushThemeToEditor(frame, EDITOR_ORIGIN, { preset, mode, overrides, overrideAlphas });
  }, [preset, mode, overrides, overrideAlphas]);

  return (
    <div className="w-full h-full flex flex-col">
      <div className="p-4 border-b border-border flex items-center gap-3">
        <h1 className="text-lg font-semibold text-text">Editor</h1>
        <span className="text-sm text-muted">LaTeX editor (TeXbrain)</span>
        <div className="flex items-center gap-2 ml-auto">
          {projects.length > 0 && (
            <select
              className="text-sm bg-panel text-text border border-border rounded px-2 py-1"
              value={currentNoteId ?? ""}
              onChange={(e) => openProject(Number(e.target.value))}
              title="Open editor project"
            >
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
            onClick={() => void handleNewProject()}
          >
            New project
          </button>
        </div>
      </div>
      {error && (
        <div className="px-4 py-2 text-sm text-danger border-b border-border">
          {error}
        </div>
      )}
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
          pushThemeToEditor(frame, EDITOR_ORIGIN, { preset, mode, overrides, overrideAlphas });
        }}
      />
    </div>
  );
}
