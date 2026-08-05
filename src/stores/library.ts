import { create } from "zustand";
import type { PaperSort } from "../api/papers";

export type LibraryFilterMode = "all" | "has_pdf" | "no_pdf";

interface LibraryState {
  search: string;
  setSearch: (search: string) => void;
  filterMode: LibraryFilterMode;
  setFilterMode: (mode: LibraryFilterMode) => void;
  sort: PaperSort;
  setSort: (sort: PaperSort) => void;
}

// Session-scoped (not persisted): the Library toolbar state survives navigating
// to a paper detail and back, but resets on a fresh app launch.
export const useLibraryStore = create<LibraryState>((set) => ({
  search: "",
  filterMode: "all",
  sort: "published_desc",

  setSearch(search) {
    set({ search });
  },

  setFilterMode(mode) {
    set({ filterMode: mode });
  },

  setSort(sort) {
    set({ sort });
  },
}));
