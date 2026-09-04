import { create } from "zustand";
import { persist } from "zustand/middleware";
import { apiFetch, type RemoteBackend } from "../api/client.ts";

// The PoC "default backend" control is UI-layer state (CONTEXT.md: Remote
// Query Mode): this store owns it, and `libraryFetch` below is the ONE place
// it becomes a request parameter. Transport (api/client.ts) holds no default
// and never reads this store.

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
      },
    }),
    { name: "linxiv-backend" }
  )
);

/** `apiFetch` addressed to the user's chosen default backend. Call this for
 *  LIBRARY queries only (papers, notes, feed, search, …) — local-concern
 *  calls (settings, storage, env: the node 403s them as operator-only) use
 *  `apiFetch` directly, which is always local unless passed a backend. */
export function libraryFetch<T>(path: string, init?: RequestInit): Promise<T> {
  return apiFetch<T>(path, init, useBackendStore.getState().defaultBackend);
}
