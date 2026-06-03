// linXiv-side bridge for the embedded TeXbrain editor (additive — no existing
// store/router is touched yet).
//
// Two pieces:
//   1. pushThemeToEditor() — resolves the active palette and posts it to the iframe
//      (the ThemeBridge host snippet). Sending RESOLVED colors (not a preset name)
//      means the guest never needs linXiv's PRESETS table and the Navy-vs-Amber
//      default mismatch is irrelevant to the wire.
//   2. EditorBridgeClient — owns the iframe message channel: replies to the
//      'texbrain:ready' handshake with theme (+ optional doc:open), exposes
//      sendDocOpen/sendCompile, and routes 'texbrain:fs' requests to an injected
//      FsResponder, replying with 'texbrain:fs:result'.
//
// Origins are pinned: the host always posts to an explicit targetOrigin and the
// client only accepts messages whose event.origin matches. Never use '*'.

import { getColors } from "./theme";
import type { ColorAlphas, PresetName, ThemeColors, ThemeMode } from "./theme";
import type {
  EditorMessage,
  FsOp,
  FsResult,
  GuestToHost,
  HostToGuest,
} from "./editorBridgeTypes";

// -------------------------------------------------------------------------
// ThemeBridge host: push the resolved 8-color palette to the embedded editor.
// -------------------------------------------------------------------------

/** The theme-store slice pushThemeToEditor needs to resolve the active palette. */
export interface ThemePushState {
  preset: PresetName;
  mode: ThemeMode;
  overrides: Partial<ThemeColors>;
  overrideAlphas: ColorAlphas;
}

/**
 * Resolve the host's current palette to concrete colors and post it to the iframe.
 * Call this inside the theme store's applyAndSet AND in reply to a 'texbrain:ready'
 * handshake so the editor is themed before first paint (no Amber flash).
 *
 * @param frame        the iframe's contentWindow (null is a no-op — frame not mounted)
 * @param targetOrigin the embedded editor's origin (EDITOR_ORIGIN); never '*'
 * @param s            the active preset/mode/overrides/overrideAlphas
 */
export function pushThemeToEditor(
  frame: Window | null,
  targetOrigin: string,
  s: ThemePushState
): void {
  if (!frame) return;
  const colors = getColors(s.preset, s.mode, s.overrides, s.overrideAlphas);
  const msg: HostToGuest = { type: "texbrain:theme", mode: s.mode, colors };
  frame.postMessage(JSON.stringify(msg), targetOrigin);
}

// -------------------------------------------------------------------------
// FS-adapter RPC responder.
// -------------------------------------------------------------------------

/**
 * The host-side handler for FS-adapter RPC. A later /api-backed adapter (mapping
 * ops onto linXiv's /api/notes CRUD, or a real on-disk vault) will implement this;
 * the signatures mirror the HostFsAdapter / FsDirHandle method surface the editor
 * consumes. Each method returns the matching FsResult variant (or throws — the
 * client serializes the error into a `texbrain:fs:result { ok: false }`).
 */
export interface FsResponder {
  /** values() — list immediate children of a directory. */
  list(path: string): Promise<Extract<FsResult, { kind: "list" }>>;
  /** getFile().text()/arrayBuffer() — read a file (base64 when binary). */
  readFile(path: string): Promise<Extract<FsResult, { kind: "readFile" }>>;
  /** createWritable().write — write a file (data is base64 when binary). */
  writeFile(
    path: string,
    data: string,
    binary: boolean
  ): Promise<Extract<FsResult, { kind: "ok" }>>;
  /** getDirectoryHandle(create) — create a directory. */
  mkdir(path: string): Promise<Extract<FsResult, { kind: "ok" }>>;
  /** removeEntry — delete a file or directory. */
  remove(
    path: string,
    recursive: boolean
  ): Promise<Extract<FsResult, { kind: "ok" }>>;
}

/**
 * Default no-op responder for the spike: lists nothing, reads empty, and accepts
 * (drops) every write/mkdir/remove. Lets the bridge run end-to-end before the real
 * /api-backed adapter exists. Swap in the real FsResponder when persistence lands.
 */
export class NoopFsResponder implements FsResponder {
  async list(): Promise<Extract<FsResult, { kind: "list" }>> {
    return { kind: "list", entries: [] };
  }
  async readFile(): Promise<Extract<FsResult, { kind: "readFile" }>> {
    return { kind: "readFile", data: "", binary: false };
  }
  async writeFile(): Promise<Extract<FsResult, { kind: "ok" }>> {
    return { kind: "ok" };
  }
  async mkdir(): Promise<Extract<FsResult, { kind: "ok" }>> {
    return { kind: "ok" };
  }
  async remove(): Promise<Extract<FsResult, { kind: "ok" }>> {
    return { kind: "ok" };
  }
}

// -------------------------------------------------------------------------
// EditorBridgeClient: owns the iframe postMessage channel.
// -------------------------------------------------------------------------

/** The document/project payload the host mounts into the editor. */
export interface DocOpenPayload {
  mainFile: string;
  files: Record<string, string>;
  projectName: string;
}

/** Pluggable host behavior. All are optional — the client has sensible defaults. */
export interface EditorBridgeHandlers {
  /**
   * Resolve the current theme to push on the 'texbrain:ready' handshake (and any
   * later manual pushTheme() call). Required to theme the editor before first paint.
   */
  getThemeState?: () => ThemePushState;
  /**
   * The document/project to mount immediately after the handshake. Return null/undefined
   * to skip the auto doc:open (the host can call sendDocOpen() later instead).
   */
  getInitialDoc?: () => DocOpenPayload | null | undefined;
  /** FS-adapter RPC handler. Defaults to NoopFsResponder. */
  fs?: FsResponder;
  /** Compile finished (or failed) on the guest. */
  onCompiled?: (status: number, log: string, pdf: number[] | null) => void;
  /** Editor buffer dirty-state changed. */
  onDirty?: (dirty: boolean) => void;
  /** Guest completed its boot handshake. */
  onReady?: () => void;
}

/**
 * Owns the host<->guest postMessage channel for one embedded TeXbrain iframe.
 *
 * Usage:
 *   const client = new EditorBridgeClient(iframe.contentWindow, EDITOR_ORIGIN, {
 *     getThemeState: () => useThemeStore.getState(),
 *     getInitialDoc: () => ({ mainFile, files, projectName }),
 *     fs: new ApiFsResponder(...),
 *     onCompiled: (status, log, pdf) => { ... },
 *   });
 *   // later, on host theme change:  client.pushTheme();
 *   // ...                            client.sendCompile();
 *   client.destroy(); // on unmount
 */
export class EditorBridgeClient {
  private readonly guest: Window | null;
  private readonly targetOrigin: string;
  private readonly handlers: EditorBridgeHandlers;
  private readonly fs: FsResponder;
  private readonly listener: (event: MessageEvent) => void;
  private disposed = false;

  /**
   * @param guest        the iframe's contentWindow (the embedded editor)
   * @param targetOrigin the editor's origin (EDITOR_ORIGIN); pinned, never '*'
   * @param handlers     host hooks (theme/doc/fs/compiled callbacks)
   */
  constructor(
    guest: Window | null,
    targetOrigin: string,
    handlers: EditorBridgeHandlers = {}
  ) {
    this.guest = guest;
    this.targetOrigin = targetOrigin;
    this.handlers = handlers;
    this.fs = handlers.fs ?? new NoopFsResponder();
    this.listener = (event: MessageEvent) => this.handleMessage(event);
    window.addEventListener("message", this.listener);
  }

  /** Remove the message listener. Call on iframe unmount. */
  destroy(): void {
    this.disposed = true;
    window.removeEventListener("message", this.listener);
  }

  // ---- senders (Host -> Guest) -----------------------------------------

  /** Mount a document/project into the editor. */
  sendDocOpen(doc: DocOpenPayload): void {
    this.post({
      type: "texbrain:doc:open",
      mainFile: doc.mainFile,
      files: doc.files,
      projectName: doc.projectName,
    });
  }

  /** Ask the editor to compile its current buffers now. */
  sendCompile(): void {
    this.post({ type: "texbrain:compile" });
  }

  /** Push the current host theme to the editor (e.g. on a theme change). */
  pushTheme(): void {
    const s = this.handlers.getThemeState?.();
    if (s) pushThemeToEditor(this.guest, this.targetOrigin, s);
  }

  // ---- inbound (Guest -> Host) -----------------------------------------

  private handleMessage(event: MessageEvent): void {
    if (this.disposed) return;
    // Pin origin + source: only accept messages from our own iframe at its origin.
    if (event.origin !== this.targetOrigin) return;
    if (this.guest && event.source !== this.guest) return;

    const msg = this.parse(event.data);
    if (!msg) return;

    switch (msg.type) {
      case "texbrain:ready":
        this.onReady();
        break;
      case "texbrain:compiled":
        this.handlers.onCompiled?.(msg.status, msg.log, msg.pdf);
        break;
      case "texbrain:dirty":
        this.handlers.onDirty?.(msg.dirty);
        break;
      case "texbrain:fs":
        void this.handleFs(msg.id, msg.op);
        break;
      default:
        // Host-authored (HostToGuest) messages echo back here in some test rigs;
        // ignore anything we don't own as an inbound case.
        break;
    }
  }

  /** Handshake reply: theme first (before first paint), then the optional doc. */
  private onReady(): void {
    this.pushTheme();
    const doc = this.handlers.getInitialDoc?.();
    if (doc) this.sendDocOpen(doc);
    this.handlers.onReady?.();
  }

  private async handleFs(id: string, op: FsOp): Promise<void> {
    try {
      const value = await this.runFsOp(op);
      this.post({ type: "texbrain:fs:result", id, ok: true, value });
    } catch (err) {
      const error = err instanceof Error ? err.message : String(err);
      this.post({ type: "texbrain:fs:result", id, ok: false, error });
    }
  }

  private runFsOp(op: FsOp): Promise<FsResult> {
    switch (op.kind) {
      case "list":
        return this.fs.list(op.path);
      case "readFile":
        return this.fs.readFile(op.path);
      case "writeFile":
        return this.fs.writeFile(op.path, op.data, op.binary ?? false);
      case "mkdir":
        return this.fs.mkdir(op.path);
      case "remove":
        return this.fs.remove(op.path, op.recursive ?? false);
    }
  }

  // ---- wire helpers ----------------------------------------------------

  private post(msg: HostToGuest): void {
    if (!this.guest) return;
    this.guest.postMessage(JSON.stringify(msg), this.targetOrigin);
  }

  /** Parse + narrow an inbound JSON-string message; returns null on anything invalid. */
  private parse(data: unknown): EditorMessage | null {
    if (typeof data !== "string") return null;
    let parsed: unknown;
    try {
      parsed = JSON.parse(data);
    } catch {
      return null;
    }
    if (
      typeof parsed === "object" &&
      parsed !== null &&
      typeof (parsed as { type?: unknown }).type === "string" &&
      (parsed as { type: string }).type.startsWith("texbrain:")
    ) {
      return parsed as EditorMessage;
    }
    return null;
  }
}

// Convenience type re-exports for host call sites that wire up the bridge.
export type { GuestToHost, HostToGuest, EditorMessage, FsOp, FsResult };
