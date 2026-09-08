/** Persisted-state migrations for the theme and ui stores, kept out of the
 *  zustand `persist` configs so they are plain data-in/data-out functions
 *  testable without a store, a DOM, or React. */
import { DEFAULT_ZOOM } from "../lib/zoom.ts";
import { DEFAULT_DENSITY, type Density } from "../lib/density.ts";

export type SidebarPageKey = "graph" | "search" | "doi" | "tags" | "notes" | "shared" | "reading";

export type SidebarPages = Record<SidebarPageKey, boolean>;

export const DEFAULT_SIDEBAR_PAGES: SidebarPages = {
  graph: true,
  search: true,
  doi: true,
  tags: false,
  notes: false,
  shared: true,
  reading: true,
};

export type ExportFormatKey = "lxproj" | "bibtex" | "obsidian";

export type ExportMethods = Record<ExportFormatKey, boolean>;

export const DEFAULT_EXPORT_METHODS: ExportMethods = {
  lxproj: true,
  bibtex: true,
  obsidian: true,
};

/** The persisted slice of the ui store (no actions). */
export interface UiPersisted {
  sidebarCollapsed: boolean;
  sidebarPages: SidebarPages;
  exportMethods: ExportMethods;
  zoom: number;
  density: Density;
  hideSingleAuthors: boolean;
}

/** ui store, v1 -> v7. */
export function migrateUi(persisted: unknown, fromVersion: number): Partial<UiPersisted> {
  const state = { ...(persisted as Partial<UiPersisted>) };
  if (fromVersion < 1) {
    state.sidebarPages = { ...DEFAULT_SIDEBAR_PAGES, ...state.sidebarPages };
  }
  if (fromVersion < 2) {
    state.exportMethods = { ...DEFAULT_EXPORT_METHODS, ...state.exportMethods };
  }
  if (fromVersion < 3) {
    // Reset is correct: zoom was introduced in v3, so no genuine pre-v3 blob carries one.
    state.zoom = DEFAULT_ZOOM;
  }
  if (fromVersion < 4) {
    // Reset is correct: hideSingleAuthors was introduced in v4; nothing older to carry.
    state.hideSingleAuthors = false;
  }
  if (fromVersion < 5) {
    // Reset is correct: density was introduced in v5; nothing older to carry.
    state.density = DEFAULT_DENSITY;
  }
  if (fromVersion < 6) {
    // Backfill the new "shared" page key into persisted sidebarPages.
    state.sidebarPages = { ...DEFAULT_SIDEBAR_PAGES, ...state.sidebarPages };
  }
  if (fromVersion < 7) {
    // Backfill the new "reading" page key into persisted sidebarPages.
    state.sidebarPages = { ...DEFAULT_SIDEBAR_PAGES, ...state.sidebarPages };
  }
  return state;
}

/** theme store, v0 -> v3. */
export function migrateTheme(stored: unknown, version: number): Record<string, unknown> {
  const s = { ...(stored as Record<string, unknown>) };
  if (version === 0) {
    delete s.glassEffects;
  }
  if (version <= 1) {
    // Reset is correct: overrideAlphas and customPalettes were introduced in v2; neither
    // field existed before, so a pre-v2 blob carrying them is corrupt, not user data.
    s.overrideAlphas = {};
    s.customPalettes = [];
  }
  if (version <= 2) {
    delete s.glassIntensity;
    delete s.glassTintColor;
    delete s.glassTintAlpha;
    if (Array.isArray(s.customPalettes)) {
      // Non-object entries pass through: destructuring null here throws, and a
      // throw inside persist rehydration bricks the store on every boot until
      // the localStorage key is cleared by hand. Stripping fields off a value
      // that has none is a no-op, not a reason to fail.
      s.customPalettes = (s.customPalettes as unknown[]).map((p) => {
        if (typeof p !== "object" || p === null) return p;
        const { glassIntensity: _gi, glassTintColor: _gtc, glassTintAlpha: _gta, ...rest } =
          p as Record<string, unknown>;
        return rest;
      });
    }
  }
  return s;
}
