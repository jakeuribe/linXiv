import { apiFetch } from "./client";

export interface TrashedPaper {
  source_fk: number;
  source_id: string;
  title: string;
  authors: string[] | null;
  published: string | null;
  deleted_at: string | null;
  pdf_path: string | null;
  had_pdf: boolean;
  project_fks: number[];
}

export interface TrashedProject {
  id: number;
  name: string;
  description: string;
  color_hex: string | null;
  project_tags: string[];
  source_ids: string[];
  paper_count: number;
  status: string;
  created_at: string | null;
  updated_at: string | null;
  archived_at: string | null;
  share_id: string | null;
  deleted_at: string | null;
}

export async function listTrash(): Promise<{ papers: TrashedPaper[]; projects: TrashedProject[] }> {
  return apiFetch<{ papers: TrashedPaper[]; projects: TrashedProject[] }>("/api/trash");
}

export interface RestorePaperResult {
  ok: boolean;
  restored: string;
  pdf_path: string | null;
  project_fks: number[];
}

export async function restorePaper(sourceId: string): Promise<RestorePaperResult> {
  return apiFetch<RestorePaperResult>(`/api/trash/${encodeURIComponent(sourceId)}/restore`, { method: "POST" });
}

export async function hardDeletePaper(sourceId: string): Promise<{ ok: boolean }> {
  return apiFetch(`/api/trash/${encodeURIComponent(sourceId)}`, { method: "DELETE" });
}

export async function restoreProject(projectId: number): Promise<{ ok: boolean }> {
  return apiFetch(`/api/trash/projects/${projectId}/restore`, { method: "POST" });
}

export async function hardDeleteProject(projectId: number): Promise<{ ok: boolean }> {
  return apiFetch(`/api/trash/projects/${projectId}`, { method: "DELETE" });
}
