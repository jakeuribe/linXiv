import { invoke } from "@tauri-apps/api/core";

export interface MpcClientStatus {
  id: string;
  name: string;
  installed: boolean;
  available: boolean;
  /** Registered command no longer exists on disk (old install); needs a reinstall. */
  stale: boolean;
  /**
   * The client's config file exists but couldn't be parsed as JSON, so
   * installed/stale are unknown (reported false). Needs manual repair —
   * distinct from "genuinely not installed".
   */
  config_error: boolean;
}

const isTauri = () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

function guard<T>(fallback: T): T | null {
  if (!isTauri()) return fallback;
  return null;
}

export async function isCliInstalled(): Promise<boolean> {
  const fb = guard(false);
  if (fb !== null) return fb;
  return invoke<boolean>("is_cli_installed");
}

export async function installCli(): Promise<void> {
  if (!isTauri()) throw new Error("Not running in Tauri");
  return invoke("install_cli");
}

export async function uninstallCli(): Promise<void> {
  if (!isTauri()) throw new Error("Not running in Tauri");
  return invoke("uninstall_cli");
}

export async function listMcpClients(): Promise<MpcClientStatus[]> {
  const fb = guard<MpcClientStatus[]>([]);
  if (fb !== null) return fb;
  return invoke<MpcClientStatus[]>("list_mcp_clients");
}

export async function installMcp(clientId: string): Promise<void> {
  if (!isTauri()) throw new Error("Not running in Tauri");
  return invoke("install_mcp", { clientId });
}

export async function uninstallMcp(clientId: string): Promise<void> {
  if (!isTauri()) throw new Error("Not running in Tauri");
  return invoke("uninstall_mcp", { clientId });
}
