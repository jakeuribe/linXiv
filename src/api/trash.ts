import { libraryFetch } from "../stores/backend.ts";
import type {
  DeletedPaperDetails as TrashedPaper,
  TrashedProjectRow as TrashedProject,
  RestoredPaper as RestorePaperResult,
  HardDeletedPaper,
  RestoredProject,
  HardDeletedProject,
} from "../types/api";

// Core's trash listing rows and restore receipt, under the UI's names.
export type { TrashedPaper, TrashedProject, RestorePaperResult };

export async function listTrash(): Promise<{ papers: TrashedPaper[]; projects: TrashedProject[] }> {
  return libraryFetch<{ papers: TrashedPaper[]; projects: TrashedProject[] }>("/api/trash");
}

export async function restorePaper(sourceId: string): Promise<RestorePaperResult> {
  return libraryFetch<RestorePaperResult>(`/api/trash/${encodeURIComponent(sourceId)}/restore`, { method: "POST" });
}

export async function hardDeletePaper(sourceId: string): Promise<HardDeletedPaper> {
  return libraryFetch(`/api/trash/${encodeURIComponent(sourceId)}`, { method: "DELETE" });
}

export async function restoreProject(projectId: number): Promise<RestoredProject> {
  return libraryFetch(`/api/trash/projects/${projectId}/restore`, { method: "POST" });
}

export async function hardDeleteProject(projectId: number): Promise<HardDeletedProject> {
  return libraryFetch(`/api/trash/projects/${projectId}`, { method: "DELETE" });
}
