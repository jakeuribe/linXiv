import { libraryFetch } from "../stores/backend.ts";
import type { PaperMembershipReceipt, Project } from "../types/api";

export async function listProjects(
  status = "active"
): Promise<{ projects: Project[] }> {
  return libraryFetch<{ projects: Project[] }>(`/api/projects?status=${status}`);
}

export async function getProject(id: number): Promise<Project> {
  return libraryFetch<Project>(`/api/projects/${id}`);
}

export interface ProjectCreateBody {
  name: string;
  description?: string;
  color_hex?: string | null;
  project_tags?: string[];
}

export async function createProject(
  body: ProjectCreateBody
): Promise<{ project: { id: number; name: string } }> {
  return libraryFetch("/api/projects", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

export interface ProjectUpdateBody {
  name?: string;
  description?: string;
  color_hex?: string | null;
  status?: string;
  project_tags?: string[];
}

export async function updateProject(
  id: number,
  body: ProjectUpdateBody
): Promise<{ ok: boolean }> {
  return libraryFetch(`/api/projects/${id}`, {
    method: "PATCH",
    body: JSON.stringify(body),
  });
}

export async function deleteProject(id: number): Promise<{ ok: boolean }> {
  return libraryFetch(`/api/projects/${id}`, { method: "DELETE" });
}

export async function archiveProject(id: number): Promise<{ ok: boolean }> {
  return updateProject(id, { status: "archived" });
}

export async function restoreProject(id: number): Promise<{ ok: boolean }> {
  return updateProject(id, { status: "active" });
}

/** Core's PaperMembershipReceipt — shared by add and remove. */
export type { PaperMembershipReceipt } from "../types/api";

export async function addPaperToProject(
  projectId: number,
  sourceId: string
): Promise<PaperMembershipReceipt> {
  return libraryFetch(`/api/projects/${projectId}/papers`, {
    method: "POST",
    body: JSON.stringify({ source_id: sourceId }),
  });
}

// Server caps source_ids at 10k per request; stay well under it.
const BULK_ADD_CHUNK = 5_000;

/** Bulk-add papers to a project. Partial success: unknown source_ids come
 *  back in `failed` while the rest are still added. Input is deduplicated
 *  and chunked, so any list size is accepted; an empty list is a no-op. */
export async function addPapersToProject(
  projectId: number,
  sourceIds: string[]
): Promise<{ ok: boolean; failed: string[] }> {
  const ids = [...new Set(sourceIds)];
  const failed: string[] = [];
  for (let i = 0; i < ids.length; i += BULK_ADD_CHUNK) {
    const res = await libraryFetch<{ ok: boolean; failed: string[] }>(
      `/api/projects/${projectId}/papers/bulk`,
      {
        method: "POST",
        body: JSON.stringify({ source_ids: ids.slice(i, i + BULK_ADD_CHUNK) }),
      }
    );
    failed.push(...res.failed);
  }
  return { ok: failed.length === 0, failed };
}

export async function removePaperFromProject(
  projectId: number,
  sourceId: string
): Promise<PaperMembershipReceipt> {
  return libraryFetch(
    `/api/projects/${projectId}/papers/${encodeURIComponent(sourceId)}`,
    { method: "DELETE" }
  );
}

export interface AddPapersVars {
  projectId: number;
  sourceIds: string[];
}

/** Resolves with the source_ids that could not be added rather than throwing,
 *  so callers can re-select exactly those and a retry can't re-add the rest. */
export async function addPapers({ projectId, sourceIds }: AddPapersVars): Promise<string[]> {
  const { failed } = await addPapersToProject(projectId, sourceIds);
  return failed;
}

export interface CreateProjectWithPapersVars {
  name: string;
  sourceIds: string[];
}

/** Same contract as addPapers: resolves with the failed ids. The project may
 *  exist even when the paper-add rejects, so that path resolves too. */
export async function createProjectWithPapers({
  name,
  sourceIds,
}: CreateProjectWithPapersVars): Promise<string[]> {
  const result = await createProject({ name });
  try {
    return await addPapers({ projectId: result.project.id, sourceIds });
  } catch {
    // The project was created; resolve so onSuccess still clears the name and
    // a retry can't create a duplicate.
    return sourceIds;
  }
}
