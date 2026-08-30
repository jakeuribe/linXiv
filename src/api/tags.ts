import { apiFetch } from "./client";
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
  return apiFetch<{ tags: TagSummary[] }>("/api/tags").then((r) => r.tags);
}

export async function getTagDetail(label: string): Promise<TagDetail> {
  return apiFetch<TagDetail>(`/api/tags/${encodeURIComponent(label)}`);
}
