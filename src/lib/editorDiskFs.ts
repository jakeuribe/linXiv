// DiskFsResponder + HostFsRouter — the embed's "Open Folder" persistence path.
//
// When the guest editor asks the host to pick a directory (texbrain:pick:folder),
// the picked folder is opened IN PLACE: every subsequent FsOp resolves against it
// through Tauri's fs plugin rather than the note vault (see the bridge comment in
// editorBridgeTypes.ts and ADR 0018 in linxiv-comprehensive-documentation — the
// import-into-vault alternative is deferred).
//
// Scope: the fs plugin only allows paths inside its scope; the dialog plugin
// extends that scope at runtime with whatever the user picks, so no static
// directory grants are needed in capabilities/default.json — just the fs
// operation permissions.
//
// HostFsRouter is what actually gets handed to EditorBridgeClient: it forwards
// each op to the disk responder while a disk root is set, else to the vault
// responder (ApiFsResponder). EditorPage owns the root: set on a successful
// pick, cleared on every vault doc:open.

import {
  lstat,
  mkdir,
  readDir,
  readFile,
  remove,
  writeFile,
  writeTextFile,
} from "@tauri-apps/plugin-fs";
import { base64ToBytes, bytesToBase64 } from "./base64.ts";
import type { FsResponder } from "./editorBridge";
import type { FsResult } from "./editorBridgeTypes";

/**
 * Join a guest-supplied project-relative path onto the picked absolute root.
 * Paths arrive over an untrusted wire (the iframe could be compromised), so
 * reject anything lexical that could escape the root. This is only half the
 * check — symlinks are invisible here — so every op goes through
 * resolvePathChecked below, never this directly.
 */
function resolvePath(root: string, rel: string): string {
  const parts = rel.split("/").filter(Boolean);
  // "." must be rejected too (filter(Boolean) keeps it): `root + "/."` defeats
  // remove()'s string-equality root guard while still naming the root. Same
  // segment rules as the plugin's zip-entry lookup (serve.rs).
  if (rel.startsWith("/") || rel.includes("\\") || parts.some((p) => p === ".." || p === ".")) {
    throw new Error(`path escapes the project root: ${rel}`);
  }
  return parts.length ? `${root}/${parts.join("/")}` : root;
}

/**
 * resolvePath + symlink refusal. The lexical checks can't see symlinks (a
 * `link -> /etc/passwd` inside the root passes them and the OS follows it on
 * read), and the fs plugin's scope check is also lexical — so lstat every
 * component under the root and refuse symlinks outright. Missing components
 * are fine (writes/mkdirs create them); any other lstat failure fails closed.
 * Best-effort: a link swapped in (or a checked component deleted and recreated
 * as a link) between this check and the op still wins the race, but that
 * already requires local write access to the picked folder.
 */
async function resolvePathChecked(root: string, rel: string): Promise<string> {
  const abs = resolvePath(root, rel); // lexical validation first (throws on "..")
  let cur = root;
  for (const part of rel.split("/").filter(Boolean)) {
    cur = `${cur}/${part}`;
    let info;
    try {
      info = await lstat(cur);
    } catch (e) {
      // Not-found is expected for to-be-created paths: every prior component
      // already passed the symlink check, and the missing component plus
      // everything under it is being created fresh, so nothing left on the
      // path can be a symlink — safe to return abs without walking further.
      // Anything else (scope/permission failure) must not silently skip the
      // check. Detection is by error STRING because tauri-plugin-fs (verified
      // at 2.5.1) serializes errors as std::io::Error's Display text — there
      // is no structured code to branch on; re-verify the format on plugin
      // upgrades (a format change fails closed here: throw, not skip).
      // os error 2 is not-found on every platform; 3 is PATH_NOT_FOUND on
      // Windows only (on Unix it's ESRCH — never treat it as not-found there).
      const text = String(e);
      const win =
        typeof navigator !== "undefined" && navigator.userAgent.includes("Windows");
      if (/\(os error 2\)/.test(text) || (win && /\(os error 3\)/.test(text))) {
        return abs;
      }
      throw e;
    }
    if (info.isSymlink) {
      throw new Error(`path escapes the project root (symlink): ${rel}`);
    }
  }
  return abs;
}

/** FsResponder over a user-picked absolute directory, via the Tauri fs plugin. */
export class DiskFsResponder implements FsResponder {
  /** @param root absolute path of the picked folder (in the fs plugin's runtime scope). */
  constructor(private readonly root: string) {}

  async list(path: string): Promise<Extract<FsResult, { kind: "list" }>> {
    const entries = await readDir(await resolvePathChecked(this.root, path));
    return {
      kind: "list",
      entries: entries.map((e) => ({
        name: e.name,
        kind: e.isDirectory ? ("directory" as const) : ("file" as const),
      })),
    };
  }

  async readFile(path: string): Promise<Extract<FsResult, { kind: "readFile" }>> {
    const bytes = await readFile(await resolvePathChecked(this.root, path));
    // Text vs binary by content, not extension: a clean strict-UTF-8 decode is
    // text (what the editor buffers expect), anything else ships as base64.
    try {
      const data = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
      return { kind: "readFile", data, binary: false };
    } catch {
      return { kind: "readFile", data: bytesToBase64(bytes), binary: true };
    }
  }

  async writeFile(
    path: string,
    data: string,
    binary: boolean
  ): Promise<Extract<FsResult, { kind: "ok" }>> {
    const abs = await resolvePathChecked(this.root, path);
    if (binary) await writeFile(abs, base64ToBytes(data));
    else await writeTextFile(abs, data);
    return { kind: "ok" };
  }

  async mkdir(path: string): Promise<Extract<FsResult, { kind: "ok" }>> {
    await mkdir(await resolvePathChecked(this.root, path), { recursive: true });
    return { kind: "ok" };
  }

  async remove(
    path: string,
    recursive: boolean
  ): Promise<Extract<FsResult, { kind: "ok" }>> {
    const abs = await resolvePathChecked(this.root, path);
    // An empty path resolves to the root itself (fine for list, fatal here):
    // remove(root, {recursive}) would wipe the user's whole picked folder.
    if (abs === this.root) {
      throw new Error("cannot remove the project root directory");
    }
    await remove(abs, { recursive });
    return { kind: "ok" };
  }
}

/**
 * Routes each FsOp to the disk responder while a disk root is set (read lazily
 * via the getter, so EditorPage can re-root/clear without rebuilding the
 * long-lived bridge — same pattern as ApiFsResponder's getNoteId), else to the
 * vault responder.
 */
export class HostFsRouter implements FsResponder {
  constructor(
    private readonly getDiskRoot: () => string | null,
    private readonly vault: FsResponder
  ) {}

  private route(): FsResponder {
    const root = this.getDiskRoot();
    return root ? new DiskFsResponder(root) : this.vault;
  }

  list(path: string) {
    return this.route().list(path);
  }
  readFile(path: string) {
    return this.route().readFile(path);
  }
  writeFile(path: string, data: string, binary: boolean) {
    return this.route().writeFile(path, data, binary);
  }
  mkdir(path: string) {
    return this.route().mkdir(path);
  }
  remove(path: string, recursive: boolean) {
    return this.route().remove(path, recursive);
  }
}
