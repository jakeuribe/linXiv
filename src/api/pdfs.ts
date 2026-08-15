import { invoke } from "@tauri-apps/api/core";
import { apiFetch } from "./client";

export interface SavedPdf {
  source_id: string;
  source_fk: number;
  title: string;
  // Always >= 1: the list endpoint skips version-0 rows (no on-disk filename).
  version: number;
  size_bytes: number;
}

export async function listSavedPdfs(): Promise<{ pdfs: SavedPdf[] }> {
  return apiFetch<{ pdfs: SavedPdf[] }>("/api/pdfs");
}

export async function deleteSavedPdf(
  sourceId: string,
): Promise<{ deleted: boolean }> {
  return apiFetch<{ deleted: boolean }>(
    `/api/pdfs/${encodeURIComponent(sourceId)}`,
    { method: "DELETE" },
  );
}

/** Open a stored PDF in the OS default viewer (Tauri only). */
export async function openPdfInSystem(path: string): Promise<void> {
  return invoke("open_pdf_in_system", { path });
}
