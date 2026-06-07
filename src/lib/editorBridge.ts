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
 * Safe default when no fs handler is configured: lists nothing, reads empty, and
 * accepts (silently drops) every write/mkdir/remove. Any bridge that needs real
 * persistence must pass an FsResponder (HostFsRouter, ApiFsResponder,
 * DiskFsResponder) via EditorBridgeHandlers.fs.
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

/**
 * Serialize a thrown value for an ok:false wire reply. Tauri plugin commands
 * reject with serialized values that are NOT Error instances — plain strings
 * (tauri-plugin-fs) or `{ kind, message }` objects (tauri-plugin-texbrain) —
 * and String() on the latter yields '[object Object]', so prefer a string
 * `message` field when one exists.
 */
function extractErrorMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "object" && err !== null) {
    const m = (err as { message?: unknown }).message;
    if (typeof m === "string") return m;
  }
  return String(err);
}

// -------------------------------------------------------------------------
// EditorBridgeClient: owns the iframe postMessage channel.
// -------------------------------------------------------------------------

/** The document/project payload the host mounts into the editor. */
export interface DocOpenPayload {
  /**
   * Stable host-side project identity (the note id). The editor keys its
   * doc:open idempotency guard on THIS, not on (projectName, mainFile): those
   * collide trivially (two "Untitled" projects, both main.tex) and a guard
   * keyed on them would mistake a real switch for a re-send — keeping the old
   * buffers mounted while the host believes the new project is open, so a save
   * writes the wrong project's content. The id is unambiguous.
   */
  projectId: number;
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
  /**
   * The guest asked for the host's NATIVE directory picker (its embedded
   * "Open Folder" — the iframe can't pick itself). Show the dialog, re-root the
   * fs responder at the chosen folder, and resolve with the folder's basename
   * (null = user cancelled). Unset ⇒ the client replies ok:false so the guest
   * surfaces an error instead of hanging.
   */
  onPickFolder?: () => Promise<string | null>;
  /** Compile finished (or failed) on the guest. `pdf` is base64-encoded. */
  onCompiled?: (status: number, log: string, pdf: string | null) => void;
  /** Editor buffer dirty-state changed. */
  onDirty?: (dirty: boolean) => void;
  /**
   * Guest completed its boot handshake. `protocol` is the bridge protocol the
   * live Editor build reports in texbrain:ready — a belt-and-suspenders runtime
   * signal only (the REAL compat gate is the release-manifest check at
   * install/update time, ADR 0017); compare against SUPPORTED_BRIDGE_PROTOCOLS
   * (src/api/editorPlugin.ts, exported from the plugin's guest-js) to show a
   * non-fatal warning on mismatch.
   */
  onReady?: (protocol?: number) => void;
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
      projectId: doc.projectId,
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
    // Pin origin + source: only accept messages from our own iframe at its
    // origin. Fail closed when guest is null (the bridge can't validate the
    // source, and event.source can itself be null for a closed window).
    if (event.origin !== this.targetOrigin) return;
    if (!this.guest || event.source !== this.guest) return;

    const msg = this.parse(event.data);
    if (!msg) return;

    switch (msg.type) {
      case "texbrain:ready":
        this.onReady(msg.protocol);
        break;
      case "texbrain:compiled":
        this.handlers.onCompiled?.(msg.status, msg.log, msg.pdf);
        break;
      case "texbrain:dirty":
        this.handlers.onDirty?.(msg.dirty);
        break;
      case "texbrain:fs":
        // parse() narrows on the 'texbrain:' type prefix only, so a malformed
        // wire message can omit id/op at runtime. Without a string id the
        // reply can't be correlated to the guest's pending request — drop it.
        // (A nullish/unknown op with a valid id still gets an ok:false reply
        // via handleFs's catch.)
        if (typeof msg.id === "string") void this.handleFs(msg.id, msg.op);
        break;
      case "texbrain:pick:folder":
        if (typeof msg.id === "string") void this.handlePickFolder(msg.id);
        break;
      default:
        // Host-authored (HostToGuest) messages echo back here in some test rigs;
        // ignore anything we don't own as an inbound case.
        break;
    }
  }

  /** Handshake reply: theme first (before first paint), then the optional doc. */
  private onReady(protocol?: number): void {
    this.pushTheme();
    const doc = this.handlers.getInitialDoc?.();
    if (doc) this.sendDocOpen(doc);
    this.handlers.onReady?.(protocol);
  }

  private async handlePickFolder(id: string): Promise<void> {
    // No handler configured = this host doesn't support folder picking. Must
    // NOT ack: the ack means "supported, dialog opening" and cancels the
    // guest's support-detection timer (see editorBridgeTypes.ts). Reject
    // immediately instead — unforced, since with no ack sent a disposed
    // bridge can fall back on the guest's own ack timeout.
    if (!this.handlers.onPickFolder) {
      this.post({ type: "texbrain:pick:folder:result", id, ok: false, error: "the host does not support folder picking" });
      return;
    }
    // Ack receipt BEFORE the long-lived native dialog opens: a missing ack is
    // the guest's only signal that the host predates pick:folder (it bounds
    // its wait on the ack, not on the dialog — see editorBridgeTypes.ts).
    this.post({ type: "texbrain:pick:folder:ack", id });
    try {
      const name = await this.handlers.onPickFolder();
      // force: the result must reach the guest even if destroy() ran while the
      // native dialog sat open — the ack above already cancelled the guest's
      // support-detection timer, so a dropped result would leave its
      // pickFolder() promise pending forever.
      this.post({ type: "texbrain:pick:folder:result", id, ok: true, name }, { force: true });
    } catch (err) {
      const error = extractErrorMessage(err);
      this.post({ type: "texbrain:pick:folder:result", id, ok: false, error }, { force: true });
    }
  }

  private async handleFs(id: string, op: FsOp): Promise<void> {
    try {
      const value = await this.runFsOp(op);
      this.post({ type: "texbrain:fs:result", id, ok: true, value });
    } catch (err) {
      const error = extractErrorMessage(err);
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
      default: {
        // parse() only checks the 'texbrain:' type prefix, so a tampered wire
        // message can carry an unknown op.kind; falling through would resolve
        // undefined and post a malformed `{ ok: true }` with no value. Throw so
        // handleFs replies ok:false. The never binding keeps the switch
        // exhaustive at compile time when FsOp grows.
        const unknown: never = op;
        throw new Error(`unknown fs op kind: ${(unknown as { kind?: string }).kind}`);
      }
    }
  }

  // ---- wire helpers ----------------------------------------------------

  private post(msg: HostToGuest, opts?: { force?: boolean }): void {
    // disposed: async handlers (handleFs/handlePickFolder) can resume after
    // destroy() — a torn-down bridge must not post stale replies to the guest.
    // force bypasses that gate for pick:folder RESULTS only: once acked, the
    // guest waits on the result without bound (see handlePickFolder).
    if (!this.guest || (this.disposed && !opts?.force)) return;
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
