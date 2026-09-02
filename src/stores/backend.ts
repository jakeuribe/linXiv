import { create } from "zustand";
import { persist } from "zustand/middleware";
import { setDefaultBackend, type RemoteBackend } from "../api/client";

// The PoC "default backend" control is UI-layer state (CONTEXT.md: Remote
// Query Mode): this store owns it and PUSHES it into the client layer via
// setDefaultBackend — transport code never reads a store.

interface BackendState {
  /** `null` = the local backend. The whole backend (not just an id) so the
   *  sidebar indicator can label it without a fetch. */
  defaultBackend: RemoteBackend | null;
  setDefault: (backend: RemoteBackend | null) => void;
}

export const useBackendStore = create<BackendState>()(
  persist(
    (set) => ({
      defaultBackend: null,
      setDefault(backend) {
        set({ defaultBackend: backend });
        setDefaultBackend(backend);
      },
    }),
    {
      name: "linxiv-backend",
      // Requests made after startup must route to the persisted default.
      onRehydrateStorage: () => (state) =>
        setDefaultBackend(state?.defaultBackend ?? null),
    }
  )
);
