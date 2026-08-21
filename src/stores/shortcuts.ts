import { create } from "zustand";
import { persist } from "zustand/middleware";

/** A user-chosen rebinding for one shortcut id. Mirrors the modifiers/key
 * shape captured off a KeyboardEvent (see captureOverride in lib/shortcuts). */
export interface ShortcutOverride {
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
  key: string;
}

interface ShortcutsState {
  overrides: Record<string, ShortcutOverride>;
  setOverride: (id: string, override: ShortcutOverride) => void;
  clearOverride: (id: string) => void;
}

export const useShortcutsStore = create<ShortcutsState>()(
  persist(
    (set) => ({
      overrides: {},

      setOverride(id, override) {
        set((state) => ({ overrides: { ...state.overrides, [id]: override } }));
      },

      clearOverride(id) {
        set((state) => {
          if (!(id in state.overrides)) return state;
          const overrides = { ...state.overrides };
          delete overrides[id];
          return { overrides };
        });
      },
    }),
    { name: "linxiv-shortcuts", version: 1 }
  )
);
