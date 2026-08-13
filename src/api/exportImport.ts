import { save, open } from "@tauri-apps/plugin-dialog";
import { join as pathJoin } from "@tauri-apps/api/path";
import { apiFetch, BASE_URL, bytesToBase64, isTauri } from "./client";
import type { BibtexImportReceipt } from "../types/api";

async function fileToBase64(file: File): Promise<string> {
  return bytesToBase64(new Uint8Array(await file.arrayBuffer()));
}

function pickerCancelled(): Error {
  return Object.assign(new Error("Cancelled"), { name: "AbortError" });
}

export interface ImportPreview {
  project_name: string;
  description: string;
  paper_count: number;
  note_count: number;
  has_pdfs: boolean;
  format_version: number;
}

async function fetchBlob(url: string, init?: RequestInit): Promise<{ blob: Blob; filename?: string }> {
  const res = await fetch(url, init);
  if (!res.ok) {
    const body = await res.json().catch(() => ({})) as { detail?: string };
    throw new Error(body.detail ?? `Request failed (${res.status})`);
  }
  const cd = res.headers.get("Content-Disposition") ?? "";
  const match = cd.match(/filename[^;=\n]*=(?:(['"])(.+?)\1|([^;\n]+))/);
  const filename = match ? (match[2] ?? match[3])?.trim() : undefined;
  return { blob: await res.blob(), filename };
}

function triggerDownload(blob: Blob, filename: string): void {
  const url = URL.createObjectURL(blob);
  const a = Object.assign(document.createElement("a"), { href: url, download: filename });
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  setTimeout(() => URL.revokeObjectURL(url), 100);
}

function slugify(name?: string, id?: number, ext = ""): string {
  const stripped = name ? name.replace(/[:/\\*?"<>|]/g, "").replace(/\s+/g, "_").toLowerCase() : "";
  const base = stripped || `project-${id ?? "unknown"}`;
  return `${base}${ext}`;
}

export async function exportProject(
  projectId: number,
  includePdfs = false,
  projectName?: string
): Promise<void> {
  const slug = slugify(projectName, projectId, ".lxproj");
  if (isTauri) {
    const destPath = await save({
      defaultPath: slug,
      filters: [{ name: "linXiv Project", extensions: ["lxproj"] }],
    });
    if (!destPath) throw pickerCancelled();
    await apiFetch(`/api/projects/${projectId}/export`, {
      method: "POST",
      body: JSON.stringify({ project_id: projectId, include_pdfs: includePdfs, dest_path: destPath }),
    });
    return;
  }
  const { blob, filename } = await fetchBlob(`${BASE_URL}/api/projects/${projectId}/export`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ project_id: projectId, include_pdfs: includePdfs }),
  });
  triggerDownload(blob, filename ?? slug);
}

export async function previewImport(file: File): Promise<ImportPreview> {
  if (isTauri) {
    return apiFetch<ImportPreview>("/api/projects/import/preview", {
      method: "POST",
      body: JSON.stringify({ file_b64: await fileToBase64(file) }),
    });
  }
  const fd = new FormData();
  fd.append("file", file);
  return apiFetch<ImportPreview>("/api/projects/import/preview", { method: "POST", body: fd });
}

export async function commitImport(
  file: File,
  onConflict: "merge" | "overwrite" = "merge"
): Promise<{ project_id: number }> {
  if (isTauri) {
    return apiFetch<{ project_id: number }>("/api/projects/import/commit", {
      method: "POST",
      body: JSON.stringify({ file_b64: await fileToBase64(file), on_conflict: onConflict }),
    });
  }
  const fd = new FormData();
  fd.append("file", file);
  return apiFetch<{ project_id: number }>(
    `/api/projects/import/commit?on_conflict=${onConflict}`,
    { method: "POST", body: fd }
  );
}

export async function exportBibtex(projectId: number, projectName?: string): Promise<void> {
  const slug = slugify(projectName, projectId, ".bib");
  if (isTauri) {
    const destPath = await save({
      defaultPath: slug,
      filters: [{ name: "BibTeX", extensions: ["bib"] }],
    });
    if (!destPath) throw pickerCancelled();
    await apiFetch(`/api/projects/${projectId}/export/bibtex?dest_path=${encodeURIComponent(destPath)}`);
    return;
  }
  const { blob } = await fetchBlob(`${BASE_URL}/api/projects/${projectId}/export/bibtex`);
  triggerDownload(blob, slug);
}

export async function exportObsidian(projectId: number, projectName?: string): Promise<void> {
  const slug = slugify(projectName, projectId, ".md");
  if (isTauri) {
    const picked = await open({ directory: true, title: "Select Obsidian vault folder" });
    const destDir = Array.isArray(picked) ? picked[0] : picked;
    if (!destDir) throw pickerCancelled();
    const destPath = await pathJoin(destDir, slug);
    await apiFetch(`/api/projects/${projectId}/export/obsidian?dest_path=${encodeURIComponent(destPath)}`);
    return;
  }
  const { blob } = await fetchBlob(`${BASE_URL}/api/projects/${projectId}/export/obsidian`);
  triggerDownload(blob, slug);
}

export async function importBibtex(
  file: File,
  projectId?: number
): Promise<BibtexImportReceipt> {
  if (isTauri) {
    const file_b64 = await fileToBase64(file);
    return apiFetch<BibtexImportReceipt>(
      "/api/papers/import/bibtex",
      {
        method: "POST",
        body: JSON.stringify(projectId ? { file_b64, project_id: projectId } : { file_b64 }),
      }
    );
  }
  const fd = new FormData();
  fd.append("file", file);
  const path = projectId
    ? `/api/papers/import/bibtex?project_id=${projectId}`
    : "/api/papers/import/bibtex";
  return apiFetch<BibtexImportReceipt>(path, { method: "POST", body: fd });
}

export async function importPdf(
  file: File,
  projectId?: number
): Promise<{ source_id: string; title: string }> {
  const path = projectId
    ? `/api/papers/import/pdf?project_id=${projectId}`
    : "/api/papers/import/pdf";
  if (isTauri) {
    return apiFetch<{ source_id: string; title: string }>(path, {
      method: "POST",
      body: JSON.stringify({ file_b64: await fileToBase64(file), filename: file.name }),
    });
  }
  const fd = new FormData();
  fd.append("file", file);
  return apiFetch<{ source_id: string; title: string }>(path, { method: "POST", body: fd });
}
