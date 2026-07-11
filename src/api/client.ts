/**
 * Backend client. In the packaged app the backend runs IN-PROCESS — requests go
 * through the `api` Tauri command (and PDFs/graph over the linxiv:// scheme), so
 * there is no HTTP base. In browser dev, Vite proxies `/api` to a dev backend
 * (D32), so an empty base URL lets the proxy handle it.
 */
export const isTauri =
  typeof window !== "undefined" && window.__TAURI_INTERNALS__ !== undefined;

// Empty base: the in-process app never builds an HTTP URL (it uses invoke +
// linxiv://); the browser-dev `fetch` path relies on the Vite `/api` proxy.
export const BASE_URL = "";

// Webviews can't send a multipart body through Tauri `invoke`, so file uploads
// travel as a base64 `file_b64` JSON field instead.
export { bytesToBase64 } from "../lib/base64.ts";

export class ApiError extends Error {
  constructor(
    public readonly status: number,
    message: string
  ) {
    super(message);
    this.name = "ApiError";
  }
}

// Packaged app: every request runs in-process through the `api` command. (Tauri
// never sends FormData here — file uploads send base64 JSON; the FormData branch
// below is the browser-dev path only.)
async function invokeApi<T>(path: string, init?: RequestInit): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  const method = (init?.method ?? "GET").toUpperCase();
  const body =
    typeof init?.body === "string" ? (JSON.parse(init.body) as unknown) : null;
  try {
    return await invoke<T>("api", { req: { method, path, body } });
  } catch (e) {
    const err = e as { status?: number; detail?: string };
    throw new ApiError(err.status ?? 500, err.detail ?? "Request failed");
  }
}

export async function apiFetch<T>(
  path: string,
  init?: RequestInit
): Promise<T> {
  if (isTauri && !(init?.body instanceof FormData)) {
    return invokeApi<T>(path, init);
  }
  const url = `${BASE_URL}${path}`;
  const isFormData = init?.body instanceof FormData;
  const response = await fetch(url, {
    ...init,
    headers: isFormData
      ? init?.headers
      : { "Content-Type": "application/json", ...init?.headers },
  });

  if (!response.ok) {
    let detail = `HTTP ${response.status}`;
    try {
      const body = (await response.json()) as { detail?: string };
      if (body.detail) detail = body.detail;
    } catch {
      // ignore parse errors
    }
    throw new ApiError(response.status, detail);
  }

  // 204 No Content or empty body
  const text = await response.text();
  if (!text) return undefined as T;
  return JSON.parse(text) as T;
}
