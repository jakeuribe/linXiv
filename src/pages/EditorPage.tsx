// EditorPage.tsx
// -----------------------------------------------------------------------------
// Host-side full-canvas view that embeds the TeXbrain editor in an <iframe> and
// drives it over the EditorBridge. Modeled on GraphPage.tsx (full-height flex:
// header bar + flex-1 iframe), it wires an EditorBridgeClient to the iframe's
// contentWindow and pushes live linXiv theme updates to the guest.
//
// This component is COMPLETE but NOT yet registered in the router/sidebar —
// that wiring lives in the `register-editor-route-and-nav` section (and a
// matching *.PATCH.md beside this file). Keeping it unimported means this
// section stays purely additive.
// -----------------------------------------------------------------------------

import { useEffect, useRef } from "react";
import { useThemeStore, registerEditorFrame } from "../stores/theme";
import {
  EditorBridgeClient,
  NoopFsResponder,
  pushThemeToEditor,
  EDITOR_ORIGIN,
  EDITOR_SRC,
  type ThemePushState,
} from "./editorConfig";

export default function EditorPage() {
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const bridgeRef = useRef<EditorBridgeClient | null>(null);

  // Subscribe to the theme store the same way GraphPage does, so live theme
  // edits re-theme the embedded editor. We read the four resolvable inputs and
  // push resolved colors over the wire (the guest never needs linXiv's PRESETS).
  const preset = useThemeStore((s) => s.preset);
  const mode = useThemeStore((s) => s.mode);
  const overrides = useThemeStore((s) => s.overrides);
  const overrideAlphas = useThemeStore((s) => s.overrideAlphas);

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
      // Spike persistence: no real FS backend yet. The /api-backed adapter
      // (HostFsAdapter contract) arrives in a later section; swap it in here.
      fs: new NoopFsResponder(),
    });
    bridgeRef.current = bridge;
    return () => {
      bridge.destroy();
      bridgeRef.current = null;
      registerEditorFrame(null);
    };
  }, []);

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
      </div>
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
        // On (re)load the guest posts 'texbrain:ready'; the bridge replies with
        // the current theme. We also nudge a theme push here in case the bridge
        // missed the handshake (e.g. a hot reload after mount).
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
