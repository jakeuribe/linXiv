import { useEffect } from "react";
import { useUiStore } from "../stores/ui.ts";
import { ZOOM_STEP, DEFAULT_ZOOM } from "./zoom.ts";

// Central inventory of the app's keyboard shortcuts. This is the single source
// of truth: the Settings "Shortcuts" view renders it, and useGlobalShortcuts()
// binds the window-level ones. Element-scoped chords (form submit) live with
// their components — they're listed here for display only (no `run`), because
// a global listener can't know which form is focused.

export type ShortcutScope = "global" | "form";

export interface Shortcut {
  id: string;
  /** Key tokens rendered as separate <kbd> chips, e.g. ["Ctrl/Cmd", "+"]. */
  keys: string[];
  description: string;
  scope: ShortcutScope;
  /** Present only for window-bound shortcuts dispatched by useGlobalShortcuts. */
  match?: (e: KeyboardEvent) => boolean;
  run?: () => void;
}

// Ctrl (Win/Linux) or Cmd (macOS), but not Alt — the zoom modifier.
const zoomMod = (e: KeyboardEvent) => (e.ctrlKey || e.metaKey) && !e.altKey;

export const SHORTCUTS: Shortcut[] = [
  {
    id: "zoom-in",
    keys: ["Ctrl/Cmd", "+"],
    description: "Zoom the interface in",
    scope: "global",
    match: (e) => zoomMod(e) && (e.key === "+" || e.key === "="),
    run: () => {
      const { zoom, setZoom } = useUiStore.getState();
      setZoom(zoom + ZOOM_STEP);
    },
  },
  {
    id: "zoom-out",
    keys: ["Ctrl/Cmd", "-"],
    description: "Zoom the interface out",
    scope: "global",
    match: (e) => zoomMod(e) && (e.key === "-" || e.key === "_"),
    run: () => {
      const { zoom, setZoom } = useUiStore.getState();
      setZoom(zoom - ZOOM_STEP);
    },
  },
  {
    id: "zoom-reset",
    keys: ["Ctrl/Cmd", "0"],
    description: "Reset the interface zoom to 100%",
    scope: "global",
    match: (e) => zoomMod(e) && e.key === "0",
    run: () => useUiStore.getState().setZoom(DEFAULT_ZOOM),
  },
  {
    id: "submit",
    keys: ["Ctrl/Cmd", "Enter"],
    description: "Submit the focused form or dialog",
    scope: "form",
  },
];

/**
 * Binds a single window keydown listener that dispatches the global shortcuts.
 * preventDefault stops the webview's own native zoom so the two don't compound;
 * the store setters already clamp + persist. Mount this once, near the app root.
 */
export function useGlobalShortcuts(): void {
  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      for (const s of SHORTCUTS) {
        if (s.run && s.match?.(e)) {
          e.preventDefault();
          s.run();
          return;
        }
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);
}
