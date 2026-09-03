import { libraryFetch } from "../stores/backend.ts";
import type { Annotation } from "../types/api";

export async function getAnnotations(
  sourceId: string,
  projectId?: number | null,
  allProjects?: boolean
): Promise<{ annotations: Annotation[] }> {
  const params = new URLSearchParams({ source_id: sourceId });
  // all_projects is an unconditional override on the backend: when set, every
  // scope is returned and project_id is ignored. Mirror that here.
  if (allProjects) {
    params.set("all_projects", "true");
  } else if (projectId !== undefined && projectId !== null) {
    params.set("project_id", String(projectId));
  }
  return libraryFetch<{ annotations: Annotation[] }>(
    `/api/annotations?${params.toString()}`
  );
}

export interface AnnotationCreateBody {
  source_id: string;
  anchor: string;
  comment?: string;
  project_id?: number | null;
}

export async function createAnnotation(
  body: AnnotationCreateBody
): Promise<{ id: number }> {
  return libraryFetch("/api/annotations", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

export async function updateAnnotation(
  id: number,
  comment: string
): Promise<{ ok: boolean }> {
  return libraryFetch(`/api/annotations/${id}`, {
    method: "PATCH",
    body: JSON.stringify({ comment }),
  });
}

export async function deleteAnnotation(id: number): Promise<{ ok: boolean }> {
  return libraryFetch(`/api/annotations/${id}`, { method: "DELETE" });
}
