import { apiFetch } from "./client";
import type { Paper, Project } from "../types/api";

export interface TagDetail {
  label: string;
  papers: Paper[];
  projects: Project[];
}

export interface TagSummary {
  label: string;
  paper_count: number;
}

export async function getAllTags(): Promise<TagSummary[]> {
  return apiFetch<{ tags: TagSummary[] }>("/api/tags").then((r) => r.tags);
}

export async function getTagDetail(label: string): Promise<TagDetail> {
  return apiFetch<TagDetail>(`/api/tags/${encodeURIComponent(label)}`);
}
