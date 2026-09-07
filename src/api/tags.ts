import { libraryFetch } from "../stores/backend.ts";
import type { TagDetail, TagsResponse, TagWithCount as TagSummary } from "../types/api";

/** Core's `TagDetail` — the `GET /api/tags/{label}` envelope. */
export type { TagDetail };

/** Core's `TagWithCount`. */
export type { TagSummary };

export async function getAllTags(): Promise<TagSummary[]> {
  return libraryFetch<TagsResponse>("/api/tags").then((r) => r.tags);
}

export async function getTagDetail(label: string): Promise<TagDetail> {
  return libraryFetch<TagDetail>(`/api/tags/${encodeURIComponent(label)}`);
}
