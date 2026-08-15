import { create } from "zustand";
import { persist } from "zustand/middleware";
import { applyTheme, VALID_HEX } from "../lib/theme";
import type { ColorAlphas, PresetName, ThemeColors, ThemeMode } from "../lib/theme";
import { pushThemeToEditor, EDITOR_ORIGIN } from "../pages/editorConfig";
import { migrateTheme } from "./migrations.ts";

const STORAGE_KEY = "linxiv-theme";

// Set by EditorPage on mount/iframe-load (cleared on unmount) so the store can
// push resolved theme colors to the embedded TeXbrain editor whenever the theme
// changes — covers programmatic applyTheme callers outside React's render cycle.
// No-op when no editor is mounted. The import graph stays acyclic: editorConfig
// imports only ../api/client + ../lib/editorBridge, neither of which imports this
// store.
let editorFrame: Window | null = null;
export function registerEditorFrame(frame: Window | null): void {
  editorFrame = frame;
}

export interface CustomPalette {
  name: string;
  preset: PresetName;
  /** Captured at save time so applying restores the intended mode. Optional for backwards compat. */
  mode?: ThemeMode;
  overrides: Partial<ThemeColors>;
  overrideAlphas: ColorAlphas;
}

interface ThemeState {
  preset: PresetName;
  mode: ThemeMode;
  overrides: Partial<ThemeColors>;
  overrideAlphas: ColorAlphas;
  customPalettes: CustomPalette[];
  setPreset: (p: PresetName) => void;
  setMode: (m: ThemeMode) => void;
  setOverride: (key: keyof ThemeColors, val: string) => void;
  removeOverride: (key: keyof ThemeColors) => void;
  setOverrideAlpha: (key: keyof ThemeColors, alpha: number) => void;
  setOverrideWithAlpha: (key: keyof ThemeColors, hex: string, alpha: number) => void;
  saveCustomPalette: (name: string) => void;
  deleteCustomPalette: (name: string) => void;
  applyCustomPalette: (palette: CustomPalette) => void;
  /** Apply server-persisted overrides + alphas in one shot (used on boot restore). */
  restoreFromSettings: (overrides: Partial<ThemeColors>, overrideAlphas: ColorAlphas) => void;
}

export type AppThemeState = ReturnType<typeof useThemeStore.getState>;

function clamp(v: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(hi, v));
}

export const useThemeStore = create<ThemeState>()(
  persist(
    (set, get) => {
      function applyAndSet(patch: Partial<ThemeState>) {
        set(patch);
        const next = get();
        applyTheme(next.preset, next.mode, next.overrides, next.overrideAlphas);
        pushThemeToEditor(editorFrame, EDITOR_ORIGIN, {
          preset: next.preset,
          mode: next.mode,
          overrides: next.overrides,
          overrideAlphas: next.overrideAlphas,
        });
      }

      return {
        preset: "Navy" as PresetName,
        mode: "dark" as ThemeMode,
        overrides: {},
        overrideAlphas: {},
        customPalettes: [],

        setPreset(p) {
          applyAndSet({ preset: p, overrides: {}, overrideAlphas: {} });
        },

        setMode(m) {
          applyAndSet({ mode: m });
        },

        setOverride(key, val) {
          if (!VALID_HEX.test(val)) return;
          const next = { ...get().overrides, [key]: val };
          applyAndSet({ overrides: next });
        },

        removeOverride(key) {
          if (!(key in get().overrides)) return;
          const nextOverrides = { ...get().overrides };
          const nextAlphas = { ...get().overrideAlphas };
          delete nextOverrides[key];
          delete nextAlphas[key];
          applyAndSet({ overrides: nextOverrides, overrideAlphas: nextAlphas });
        },

        setOverrideAlpha(key, alpha) {
          if (get().overrides[key] === undefined) return;
          const next = { ...get().overrideAlphas, [key]: clamp(alpha, 0, 100) };
          applyAndSet({ overrideAlphas: next });
        },

        setOverrideWithAlpha(key, hex, alpha) {
          if (!VALID_HEX.test(hex)) return;
          const nextOverrides = { ...get().overrides, [key]: hex };
          const nextAlphas = { ...get().overrideAlphas, [key]: clamp(alpha, 0, 100) };
          applyAndSet({ overrides: nextOverrides, overrideAlphas: nextAlphas });
        },

        saveCustomPalette(name) {
          const { preset, mode, overrides, overrideAlphas, customPalettes } = get();
          const palette: CustomPalette = {
            name,
            preset,
            mode,
            overrides: { ...overrides },
            overrideAlphas: { ...overrideAlphas },
          };
          const nameLower = name.toLowerCase();
          const idx = customPalettes.findIndex((p) => p.name.toLowerCase() === nameLower);
          if (idx === -1) {
            set({ customPalettes: [...customPalettes, palette] });
          } else {
            const next = [...customPalettes];
            next[idx] = palette;
            set({ customPalettes: next });
          }
        },

        deleteCustomPalette(name) {
          const lower = name.toLowerCase();
          set({ customPalettes: get().customPalettes.filter((p) => p.name.toLowerCase() !== lower) });
        },

        applyCustomPalette(palette) {
          applyAndSet({
            preset: palette.preset,
            mode: palette.mode ?? get().mode,
            overrides: { ...palette.overrides },
            overrideAlphas: { ...palette.overrideAlphas },
          });
        },

        restoreFromSettings(overrides, overrideAlphas) {
          const safeOverrides: Partial<ThemeColors> = {};
          for (const k of Object.keys(overrides) as Array<keyof ThemeColors>) {
            const v = overrides[k];
            if (v && VALID_HEX.test(v)) safeOverrides[k] = v;
          }
          const safeAlphas: ColorAlphas = {};
          for (const k of Object.keys(overrideAlphas) as Array<keyof ThemeColors>) {
            const v = overrideAlphas[k];
            if (typeof v === "number") safeAlphas[k] = clamp(v, 0, 100);
          }
          applyAndSet({ overrides: safeOverrides, overrideAlphas: safeAlphas });
        },
      };
    },
    {
      name: STORAGE_KEY,
      version: 3,
      migrate: migrateTheme,
      onRehydrateStorage: () => (state) => {
        if (state) {
          applyTheme(state.preset, state.mode, state.overrides, state.overrideAlphas);
        }
      },
    }
  )
);
