import { create } from "zustand";
import { persist } from "zustand/middleware";
import { cycleStatus, type ReadingStatus } from "../lib/readingStatus";

// ponytail: statuses persist to localStorage keyed by paper source_id. This
// store is the swap point for the backend paper↔reading-status table: replace
// persistence with API calls + react-query invalidation when endpoints land.
interface ReadingStatusState {
  statuses: Record<string, ReadingStatus>;
  cycle: (sourceId: string) => void;
  remove: (sourceId: string) => void;
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
