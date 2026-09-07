// Host API client for the embedded TeXbrain editor: editor projects (the note-link)
// and the on-disk vault filesystem RPC. Mirrors the backend routes in api/app.py
// (/api/editor/...). The vault FS op forwards a single FsOp and returns its FsResult,
// so ApiFsResponder (src/lib/editorFsResponder.ts) is a thin wrapper over vaultFsOp.

import { libraryFetch } from "../stores/backend.ts";
import type { EditorProjectSummary, EditorProjectsResponse } from "../types/api";
import type { FsOp, FsResult } from "../lib/editorBridgeTypes";
import type { DocOpenPayload } from "../lib/editorBridge";

/** One editor project = a frontmatter-flagged NOTE owning a vault at note_<noteId>/. */
export type { EditorProjectSummary };

/** List editor projects, optionally scoped to a linXiv project. */
export async function listEditorProjects(
  projectId?: number | null
): Promise<EditorProjectSummary[]> {
  const q = projectId != null ? `?project_id=${projectId}` : "";
  const res = await libraryFetch<EditorProjectsResponse>(
    `/api/editor/projects${q}`
  );
  return res.projects;
}

export interface CreateEditorProjectBody {
  project_name: string;
  main_file?: string;
  /** Paper this project is "about". Omitted ⇒ standalone (sentinel paper root). */
  source_id?: string | null;
  /** Optional linXiv project scope. */
  project_id?: number | null;
}

/** Create an editor project (note + scaffolded vault). Returns the new note id. */
export async function createEditorProject(
  body: CreateEditorProjectBody
): Promise<{ noteId: number; projectName: string; mainFile: string }> {
  return libraryFetch("/api/editor/projects", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

/**
 * The doc the host pushes to the editor: main file + all text files. The
 * backend doesn't know the bridge's `projectId` (that IS the noteId); the
 * caller stamps it on before sendDocOpen, so this returns the payload sans id.
 */
export async function getEditorDoc(
  noteId: number
): Promise<Omit<DocOpenPayload, "projectId">> {
  return libraryFetch(`/api/editor/projects/${noteId}/doc`);
}

/** Forward one FsOp to the vault; resolves with the FsResult or throws ApiError. */
export async function vaultFsOp(noteId: number, op: FsOp): Promise<FsResult> {
  return libraryFetch(`/api/editor/vault/${noteId}/fs`, {
    method: "POST",
    body: JSON.stringify(op),
  });
}
