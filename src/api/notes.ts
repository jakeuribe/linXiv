import { libraryFetch } from "../stores/backend.ts";
import type {
  DeletedNote,
  Note,
  NoteGetResponse,
  NoteListResponse,
} from "../types/api";

export async function getNotes(
  sourceId: string,
  projectId?: number | null,
  allProjects?: boolean
): Promise<NoteListResponse> {
  const params = new URLSearchParams({ source_id: sourceId });
  // all_projects is an unconditional override on the backend: when set, every
  // scope is returned and project_id is ignored. Mirror that here so a caller
  // passing both doesn't send a misleading project_id that has no effect.
  if (allProjects) {
    params.set("all_projects", "true");
  } else if (projectId !== undefined && projectId !== null) {
    params.set("project_id", String(projectId));
  }
  return libraryFetch<NoteListResponse>(`/api/notes?${params.toString()}`);
}

export async function getNote(id: number): Promise<NoteGetResponse> {
  return libraryFetch<NoteGetResponse>(`/api/notes/${id}`);
}

export interface NoteCreateBody {
  source_id: string;
  project_id?: number | null;
  title?: string;
  content?: string;
}

export async function createNote(body: NoteCreateBody): Promise<Note> {
  return libraryFetch("/api/notes", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

export interface NoteUpdateBody {
  title?: string;
  content?: string;
}

export async function updateNote(
  id: number,
  body: NoteUpdateBody
): Promise<Note> {
  return libraryFetch(`/api/notes/${id}`, {
    method: "PATCH",
    body: JSON.stringify(body),
  });
}

export async function deleteNote(id: number): Promise<DeletedNote> {
  return libraryFetch(`/api/notes/${id}`, { method: "DELETE" });
}
