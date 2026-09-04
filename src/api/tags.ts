import { libraryFetch } from "../stores/backend.ts";
import type { Paper, Project, TagWithCount as TagSummary } from "../types/api";

// Envelope assembled inline by route/tags.rs — no core struct to generate.
export interface TagDetail {
  label: string;
  papers: Paper[];
  projects: Project[];
}

/** Core's `TagWithCount`. */
export type { TagSummary };

export async function getAllTags(): Promise<TagSummary[]> {
  return libraryFetch<{ tags: TagSummary[] }>("/api/tags").then((r) => r.tags);
}

export async function getTagDetail(label: string): Promise<TagDetail> {
  return libraryFetch<TagDetail>(`/api/tags/${encodeURIComponent(label)}`);
}
