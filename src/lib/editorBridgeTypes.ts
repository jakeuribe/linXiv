// Vendored wire-protocol + theme-bridge types for the embedded TeXbrain editor.
//
// These are copied verbatim from the shared embed contract (the single source of
// truth — see tex-brain's src/lib/embed/contract.ts EditorBridge / ThemeBridge
// definitions) so the linXiv host compiles WITHOUT a cross-repo import. Keep them
// byte-for-byte in sync with that contract; the only liberty taken here is
// re-exporting ThemeColors / ThemeMode from linXiv's canonical ../lib/theme so the
// 8-color palette model stays identical across the seam (per the contract note
// "keep ThemeColors identical to src/lib/theme.ts").
//
// Wire protocol over window.postMessage(JSON.stringify(msg), targetOrigin).
// Guest = embedded TeXbrain editor; Host = linXiv React shell. Host is authoritative.
// All messages carry a type prefixed 'texbrain:'. Pin targetOrigin/event.origin —
// never use '*'.

import type { ThemeColors, ThemeMode } from "./theme";

// Re-export so bridge consumers can pull the palette types from one place without
// reaching back into ../lib/theme themselves.
export type { ThemeColors, ThemeMode };

// ---- FS-adapter RPC payloads ---------------------------------------
// `op` mirrors the HostFsAdapter (FsDirHandle / FsFileHandle) method surface the
// embedded editor consumes; the host resolves each op against linXiv's /api (or disk).

export type FsOp =
  | { kind: "list"; path: string } // values()
  | { kind: "readFile"; path: string } // getFile().text()/arrayBuffer()
  | { kind: "writeFile"; path: string; data: string; binary?: boolean } // createWritable().write
  | { kind: "mkdir"; path: string } // getDirectoryHandle(create)
  | { kind: "remove"; path: string; recursive?: boolean }; // removeEntry

export type FsResult =
  | { kind: "list"; entries: Array<{ name: string; kind: "file" | "directory" }> }
  | { kind: "readFile"; data: string; binary: boolean }
  | { kind: "ok" };

// ---- Guest -> Host -------------------------------------------------

export type GuestToHost =
  // Posted once on iframe mount; host replies with texbrain:theme + (optionally) doc:open.
  | { type: "texbrain:ready"; protocol: 1 }
  // Compile finished (or failed); pdf transferred as bytes, status 0 = first-pass clean.
  | { type: "texbrain:compiled"; status: number; log: string; pdf: number[] | null }
  // Editor buffer changed (dirty tracking on the host, optional).
  | { type: "texbrain:dirty"; dirty: boolean }
  // FS-adapter RPC: guest asks host to perform an FsDirHandle/FsFileHandle op.
  | { type: "texbrain:fs"; id: string; op: FsOp };

// ---- Host -> Guest -------------------------------------------------

export type HostToGuest =
  // Push the active document/project. files keyed by repo-relative path.
  | { type: "texbrain:doc:open"; mainFile: string; files: Record<string, string>; projectName: string }
  // Ask the guest to compile the current buffers now.
  | { type: "texbrain:compile" }
  // Resolve a pending texbrain:fs request.
  | { type: "texbrain:fs:result"; id: string; ok: true; value: FsResult }
  | { type: "texbrain:fs:result"; id: string; ok: false; error: string }
  // Theme push (resolved 8 colors; see ThemeBridge contract).
  | { type: "texbrain:theme"; mode: ThemeMode; colors: ThemeColors };

export type EditorMessage = GuestToHost | HostToGuest;

// Lifecycle: mount iframe -> guest posts {ready} -> host replies {theme} then {doc:open}
// -> on demand host posts {compile}; guest replies {compiled, pdf}. FS ops are
// request/response correlated by `id`.
