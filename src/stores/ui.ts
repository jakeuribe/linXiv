import { create } from "zustand";
import { persist } from "zustand/middleware";
import { applyZoom, clampZoom, DEFAULT_ZOOM } from "../lib/zoom.ts";
import { applyDensity, normalizeDensity, DEFAULT_DENSITY, type Density } from "../lib/density.ts";
import {
  DEFAULT_EXPORT_METHODS,
  DEFAULT_SIDEBAR_PAGES,
  migrateUi,
  type ExportFormatKey,
  type ExportMethods,
  type SidebarPageKey,
  type SidebarPages,
} from "./migrations.ts";

export type { ExportFormatKey, ExportMethods, SidebarPageKey, SidebarPages };

interface UiState {
  sidebarCollapsed: boolean;
  toggleSidebar: () => void;
  sidebarPages: SidebarPages;
  setSidebarPage: (page: SidebarPageKey, enabled: boolean) => void;
  exportMethods: ExportMethods;
  setExportMethod: (format: ExportFormatKey, enabled: boolean) => void;
  zoom: number;
  setZoom: (zoom: number) => void;
  density: Density;
  setDensity: (density: Density) => void;
  hideSingleAuthors: boolean;
  setHideSingleAuthors: (hide: boolean) => void;
}

export const useUiStore = create<UiState>()(
  persist(
    (set) => ({
      sidebarCollapsed: false,
      sidebarPages: DEFAULT_SIDEBAR_PAGES,
      exportMethods: DEFAULT_EXPORT_METHODS,
      zoom: DEFAULT_ZOOM,
      density: DEFAULT_DENSITY,
      hideSingleAuthors: false,

      toggleSidebar() {
        set((state) => ({ sidebarCollapsed: !state.sidebarCollapsed }));
      },

      setSidebarPage(page, enabled) {
        set((state) => ({
          sidebarPages: { ...state.sidebarPages, [page]: enabled },
        }));
      },

      setExportMethod(format, enabled) {
        set((state) => ({
          exportMethods: { ...state.exportMethods, [format]: enabled },
        }));
      },

      setZoom(zoom) {
        const next = clampZoom(zoom);
        set({ zoom: next });
        applyZoom(next);
      },

      setDensity(density) {
        const next = normalizeDensity(density);
        set({ density: next });
        applyDensity(next);
      },

      setHideSingleAuthors(hide) {
        set({ hideSingleAuthors: hide });
      },
    }),
    {
      name: "linxiv-ui",
      version: 7,
      migrate: migrateUi,
      // The webview starts every launch at 100%; re-apply the saved zoom once
      // the persisted value is loaded (and normalize it in case the stored
      // number is out of range or corrupt).
      onRehydrateStorage: () => (state) => {
        if (state) {
          state.zoom = clampZoom(state.zoom);
          applyZoom(state.zoom);
          state.density = normalizeDensity(state.density);
          applyDensity(state.density);
        }
      },
    }
  )
);
