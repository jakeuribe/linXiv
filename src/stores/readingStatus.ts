import { create } from "zustand";
import { persist } from "zustand/middleware";
import { cycleStatus, migrateStatus, type ReadingStatus } from "../lib/readingStatus";

// Statuses persist to localStorage keyed by paper source_id. This store is the
// swap point for the backend paper↔reading-status table.
interface ReadingStatusState {
  statuses: Record<string, ReadingStatus>;
  cycle: (sourceId: string) => void;
  remove: (sourceId: string) => void;
  /** Re-key a merged-away paper's status onto the surviving paper. */
  migrate: (fromId: string, toId: string) => void;
}

export const useReadingStatusStore = create<ReadingStatusState>()(
  persist(
    (set) => ({
      statuses: {},
      cycle(sourceId) {
        set((state) => {
          const next = cycleStatus(state.statuses[sourceId]);
          const statuses = { ...state.statuses };
          if (next === undefined) delete statuses[sourceId];
          else statuses[sourceId] = next;
          return { statuses };
        });
      },
      remove(sourceId) {
        set((state) => {
          const statuses = { ...state.statuses };
          delete statuses[sourceId];
          return { statuses };
        });
      },
      migrate(fromId, toId) {
        set((state) => ({ statuses: migrateStatus(state.statuses, fromId, toId) }));
      },
    }),
    {
      name: "linxiv-reading-status",
      version: 1,
      onRehydrateStorage: () => (state, error) => {
        if (error) console.error(error);
        if (state) {
          if (typeof state.statuses !== "object" || state.statuses === null || state.statuses.constructor !== Object) {
            state.statuses = {};
          } else {
            // Filter out entries with invalid values (only "reading" and "read" are valid).
            for (const key in state.statuses) {
              if (state.statuses[key] !== "reading" && state.statuses[key] !== "read") {
                delete state.statuses[key];
              }
            }
          }
        }
      },
    }
  )
);
