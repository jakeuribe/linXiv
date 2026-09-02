import type { RemoteBackend } from "../api/client";

/** Sidebar remote-mode indicator text; `null` (local) renders nothing. */
export function remoteIndicatorLabel(
  backend: { label: string } | null
): string | null {
  if (!backend) return null;
  return backend.label.trim() || "Remote backend";
}

/** Default backend after removing backend `removedId`: removing the current
 *  default falls back to local, anything else leaves it alone. */
export function defaultAfterRemove(
  current: RemoteBackend | null,
  removedId: string
): RemoteBackend | null {
  return current?.id === removedId ? null : current;
}
