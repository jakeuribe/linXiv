# host-editor-iframe-component — deferred edits to EXISTING files

This section is **additive and unwired**: it only creates
`src/pages/EditorPage.tsx` and `src/pages/editorConfig.ts`. Per the section
rules I did **not** edit any existing file. The changes below are required by
other sections (and by the ThemeBridge contract) and are captured here as exact
diffs so the later, riskier sections can apply them verbatim.

---

## 1. Wire `pushThemeToEditor` into the theme store (ThemeBridge contract)

The ThemeBridge contract says the host must, inside `applyAndSet`, also push the
resolved theme to the embedded editor's `contentWindow`. The store does not
hold a reference to the iframe, so the cleanest non-invasive wiring is a tiny
registration hook: `EditorPage` registers its iframe window on mount, and
`applyAndSet` pushes to it if present. This keeps the store decoupled from React
and is a no-op until an editor is actually mounted.

Apply when the `register-editor-route-and-nav` section lands (so a live editor
exists to receive pushes). `EditorPage`/`editorConfig` already push on every
store change via the React effect, so this store-side push is the belt-and-
suspenders path that also covers programmatic `applyTheme` callers outside the
component's render cycle.

```diff
--- a/src/stores/theme.ts
+++ b/src/stores/theme.ts
@@
 import { create } from "zustand";
 import { persist } from "zustand/middleware";
 import { applyTheme, VALID_HEX } from "../lib/theme";
 import type { ColorAlphas, PresetName, ThemeColors, ThemeMode } from "../lib/theme";
+import { pushThemeToEditor, EDITOR_ORIGIN } from "../pages/editorConfig";

 const STORAGE_KEY = "linxiv-theme";

+// Set by EditorPage on mount (and cleared on unmount) so the store can push
+// resolved theme colors to the embedded TeXbrain editor whenever the theme
+// changes. No-op when no editor is mounted.
+let editorFrame: Window | null = null;
+export function registerEditorFrame(frame: Window | null): void {
+  editorFrame = frame;
+}
@@
       function applyAndSet(patch: Partial<ThemeState>) {
         set(patch);
         const next = get();
         applyTheme(next.preset, next.mode, next.overrides, next.overrideAlphas);
+        pushThemeToEditor(editorFrame, EDITOR_ORIGIN, {
+          preset: next.preset,
+          mode: next.mode,
+          overrides: next.overrides,
+          overrideAlphas: next.overrideAlphas,
+        });
       }
```

And the matching call in `EditorPage.tsx`'s mount effect (register on mount,
clear on unmount). This is an edit to the **new** file and may instead be folded
in directly when section 2 wires the route:

```diff
--- a/src/pages/EditorPage.tsx
+++ b/src/pages/EditorPage.tsx
@@
-import { useThemeStore } from "../stores/theme";
+import { useThemeStore, registerEditorFrame } from "../stores/theme";
@@
   useEffect(() => {
     const frame = iframeRef.current?.contentWindow ?? null;
+    registerEditorFrame(frame);
     const bridge = new EditorBridgeClient(frame, EDITOR_ORIGIN, {
@@
     return () => {
       bridge.destroy();
       bridgeRef.current = null;
+      registerEditorFrame(null);
     };
   }, []);
```

> Note: this introduces a `theme.ts -> pages/editorConfig.ts` import.
> `editorConfig` imports `../api/client` and re-exports from `../lib/editorBridge`
> (which imports only `../lib/theme`); none of them import the theme store, so the
> graph stays acyclic. If a cycle is ever a concern, import `pushThemeToEditor`
> directly from `../lib/editorBridge` and inline the `EDITOR_ORIGIN` constant.

---

## 2. Register the route + nav (separate section: `register-editor-route-and-nav`)

Not applied here (explicitly a different, riskier section). For reference, the
expected edits are:

```diff
--- a/src/App.tsx
+++ b/src/App.tsx
@@
 const PdfPreviewPage = lazy(() => import("./pages/PdfPreviewPage"));
+const EditorPage = lazy(() => import("./pages/EditorPage"));
@@
       { path: "pdf-preview", element: <PdfPreviewPage /> },
+      { path: "editor", element: <EditorPage /> },
     ],
```

```diff
--- a/src/components/layout/Sidebar.tsx
+++ b/src/components/layout/Sidebar.tsx
@@ NAV_ITEMS
+  { to: "/editor", label: "Editor", icon: <FileCode size={16} /> },
```

If the editor must preserve its buffer/PDF across navigation, prefer the
GraphPage keep-alive pattern instead (eager import + `KEEP_ALIVE` entry +
display toggle in `AppShell.tsx`) rather than the lazy child route above.
