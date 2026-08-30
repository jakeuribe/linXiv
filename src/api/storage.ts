import { save, open } from "@tauri-apps/plugin-dialog";
import { apiFetch } from "./client";
import type { BackupInfo } from "../types/api";

export type { BackupInfo };

/** Pick a destination with the OS save dialog and snapshot the DB there.
 *  Returns null when the user cancels the dialog. */
export async function backupDatabase(): Promise<BackupInfo | null> {
  const now = new Date();
  // UTC, not local time: filenames should be consistent regardless of the user's timezone.
  const destPath = await save({
    title: "Back up library to…",
    defaultPath: `linxiv-backup-${now.getUTCFullYear()}-${String(now.getUTCMonth() + 1).padStart(2, '0')}-${String(now.getUTCDate()).padStart(2, '0')}.db`,
    filters: [{ name: "SQLite database", extensions: ["db"] }],
  });
  if (!destPath) return null;
  return apiFetch<BackupInfo>("/api/storage/backup", {
    method: "POST",
    body: JSON.stringify({ dest_path: destPath }),
  });
}

/** Pick a snapshot with the OS open dialog and replace the live DB.
 *  Returns null when the user cancels the dialog. */
export async function restoreDatabase(): Promise<true | null> {
  const srcPath = await open({
    title: "Restore library from backup",
    filters: [{ name: "SQLite database", extensions: ["db"] }],
  });
  if (!srcPath) return null;
  await apiFetch("/api/storage/restore", {
    method: "POST",
    body: JSON.stringify({ src_path: srcPath }),
  });
  return true;
}
