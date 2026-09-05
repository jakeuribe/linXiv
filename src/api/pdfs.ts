import { invoke } from "@tauri-apps/api/core";
// Settings -> Storage manages the LOCAL disk (saved-PDF files and their
// linxiv:// links), so these never follow a remote default backend.
import { apiFetch } from "./client";

// Assembled inline by route/pdfs.rs (paper rows + fs metadata) — no core
// struct to generate.
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
