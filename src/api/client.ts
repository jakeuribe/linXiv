/**
 * Base HTTP client. In Tauri the backend runs at http://127.0.0.1:{port};
 * in browser dev Vite proxies /api → http://127.0.0.1:8000, so we use
 * an empty base URL and let the proxy handle it.
 *
 * In Tauri, main.tsx resolves the actual API port via the `get_api_port`
 * command at startup and calls setApiPort() before React mounts.
 */
export const isTauri =
  typeof window !== "undefined" && window.__TAURI_INTERNALS__ !== undefined;

// IMPORTANT: BASE_URL is mutable — setApiPort() updates it after the bootstrap
// resolves the Tauri-assigned port. ES module imports are live bindings, so
// downstream consumers that read this *inside a function body* see the updated
// value (verified for apiFetch, getPaperPdfUrl, and exportImport.ts).
// DO NOT capture BASE_URL into a module-level const at import time — that snapshot
// will hold the placeholder 8000 forever and silently break on machines where
// that port is taken.
export let BASE_URL = isTauri ? "http://127.0.0.1:8000" : "";

export function setApiPort(port: number): void {
  BASE_URL = isTauri ? `http://127.0.0.1:${port}` : "";
}

// Webviews can't send a multipart body through Tauri `invoke`, so file uploads
// travel as a base64 `file_b64` JSON field instead. Chunked btoa avoids the
// call-stack overflow of String.fromCharCode(...hugeArray).
export function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  const chunk = 0x8000; // 32KB
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

export class ApiError extends Error {
  constructor(
    public readonly status: number,
    message: string
  ) {
    super(message);
    this.name = "ApiError";
  }
}

// In the packaged app the backend runs in-process: route through the `api`
// Tauri command to linxiv-core instead of HTTP. A 501 from the router means that
// route isn't ported in-process yet (staged port) — we fall back to the still-
// running Python sidecar via the fetch path below. FormData uploads (3 routes)
// and browser dev also keep that path: uploads migrate to dedicated commands in
// Phase 5c, and the dev loop keeps the Vite proxy (D32). Phase 6 deletes both the
// sidecar and this fallback once every route is in-process.
const NOT_ROUTED = Symbol("not_routed");

async function invokeApi<T>(
  path: string,
  init?: RequestInit
): Promise<T | typeof NOT_ROUTED> {
  const { invoke } = await import("@tauri-apps/api/core");
  const method = (init?.method ?? "GET").toUpperCase();
  const body =
    typeof init?.body === "string" ? (JSON.parse(init.body) as unknown) : null;
  try {
    return await invoke<T>("api", { req: { method, path, body } });
  } catch (e) {
    const err = e as { status?: number; detail?: string };
    if (err.status === 501) return NOT_ROUTED; // not ported yet → fall back
    throw new ApiError(err.status ?? 500, err.detail ?? "Request failed");
  }
}

export async function apiFetch<T>(
  path: string,
  init?: RequestInit
): Promise<T> {
  if (isTauri && !(init?.body instanceof FormData)) {
    const routed = await invokeApi<T>(path, init);
    if (routed !== NOT_ROUTED) return routed;
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
