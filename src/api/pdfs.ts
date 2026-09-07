import { invoke } from "@tauri-apps/api/core";
// Settings -> Storage manages the LOCAL disk (saved-PDF files and their
// linxiv:// links), so these never follow a remote default backend.
import { apiFetch } from "./client";
import type { SavedPdf, SavedPdfListing, DeletedPdf } from "../types/api";

export type { SavedPdf };

export async function listSavedPdfs(): Promise<SavedPdfListing> {
  return apiFetch<SavedPdfListing>("/api/pdfs");
}

export async function deleteSavedPdf(
  sourceId: string,
): Promise<DeletedPdf> {
  return apiFetch<DeletedPdf>(
    `/api/pdfs/${encodeURIComponent(sourceId)}`,
    { method: "DELETE" },
  );
}

/** Open a stored PDF in the OS default viewer (Tauri only). */
export async function openPdfInSystem(path: string): Promise<void> {
  return invoke("open_pdf_in_system", { path });
}
