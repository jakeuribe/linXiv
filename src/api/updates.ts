import { isTauri } from "./client";

// linXiv ships its releases on GitHub, so "check for updates" compares the
// installed version against the latest published GitHub Release. 
const REPO = "linxiv-dev/linXiv";
const LATEST_RELEASE_API = `https://api.github.com/repos/${REPO}/releases/latest`;
export const RELEASES_PAGE = `https://github.com/${REPO}/releases`;

const REQUEST_TIMEOUT_MS = 10_000;

export interface UpdateResult {
  /** Installed version, or null when it can't be determined (browser dev build). */
  current: string | null;
  /** Latest published release, or null when none exist or the check failed. */
  latest: string | null;
  /** True only when both versions are known and the latest is strictly newer. */
  hasUpdate: boolean;
  /** Where to send the user to get the update. */
  releaseUrl: string;
  /** Human-readable failure reason, set only when the check could not complete. */
  error?: string;
}

/**
 * The installed app version. Only the packaged desktop build knows this (it
 * comes from tauri.conf.json via the app plugin); the browser dev server has no
 * such notion, so it returns null and the UI degrades to "can't compare".
 */
export async function getCurrentVersion(): Promise<string | null> {
  if (!isTauri) return null;
  try {
    const { getVersion } = await import("@tauri-apps/api/app");
    return await getVersion();
  } catch {
    return null;
  }
}

function stripLeadingV(version: string): string {
  return version.replace(/^v/i, "").trim();
}

/**
 * Compare two semver-ish strings. Returns 1 if `a` is newer than `b`, -1 if
 * older, 0 if equal or unparseable. Only the numeric `major.minor.patch` core
 * drives the result; a build with a pre-release tag (e.g. 1.2.0-rc1) ranks
 * below the same core without one. An unparseable component yields 0 so a
 * malformed tag can never masquerade as an available update.
 */
export function compareVersions(a: string, b: string): number {
  const parse = (v: string) => {
    // Split on the FIRST hyphen only — keep the whole pre-release tag intact.
    // (String.split with a limit truncates trailing segments instead of packing
    // them, so "1.2.0-beta-2" would otherwise lose its "-2".)
    const s = stripLeadingV(v);
    const dash = s.indexOf("-");
    const core = dash < 0 ? s : s.slice(0, dash);
    const pre = dash < 0 ? "" : s.slice(dash + 1);
    const nums = core.split(".").map((n) => Number.parseInt(n, 10));
    return { nums, pre };
  };
  const pa = parse(a);
  const pb = parse(b);
  const len = Math.max(pa.nums.length, pb.nums.length);
  for (let i = 0; i < len; i++) {
    const na = pa.nums[i] ?? 0;
    const nb = pb.nums[i] ?? 0;
    if (Number.isNaN(na) || Number.isNaN(nb)) return 0;
    if (na !== nb) return na > nb ? 1 : -1;
  }
  if (pa.pre && !pb.pre) return -1;
  if (!pa.pre && pb.pre) return 1;
  // Two pre-releases sharing a core: don't attempt to order them. Lexical
  // comparison is wrong (it puts rc10 below rc9), and full semver pre-release
  // ordering would be dead code here — GitHub's "latest release" excludes
  // pre-releases, so `latest` is always a stable build. Reporting equal is the
  // safe direction: it can never surface a spurious update.
  return 0;
}

/**
 * Query GitHub for the latest release and decide whether it's newer than the
 * installed build. Never throws: every failure mode (offline, rate-limited, no
 * releases yet, unknown local version) maps to a populated `UpdateResult` the
 * caller can render directly.
 */
export async function checkForUpdates(): Promise<UpdateResult> {
  const current = await getCurrentVersion();
  const base: UpdateResult = { current, latest: null, hasUpdate: false, releaseUrl: RELEASES_PAGE };

  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS);
  let resp: Response;
  try {
    resp = await fetch(LATEST_RELEASE_API, {
      headers: { Accept: "application/vnd.github+json" },
      signal: controller.signal,
    });
  } catch {
    return { ...base, error: "Couldn't reach GitHub. Check your connection and try again." };
  } finally {
    clearTimeout(timer);
  }

  // GitHub returns 404 from releases/latest when there is no published, stable
  // release — that's "nothing to update to", not an error.
  if (resp.status === 404) {
    return base;
  }
  if (!resp.ok) {
    return { ...base, error: `GitHub returned ${resp.status}. Try again later.` };
  }

  let data: { tag_name?: string; html_url?: string };
  try {
    data = await resp.json();
  } catch {
    return { ...base, error: "GitHub sent an unexpected response. Try again later." };
  }

  const tag = data.tag_name?.trim();
  if (!tag) return base;

  const latest = stripLeadingV(tag);
  const releaseUrl = data.html_url ?? RELEASES_PAGE;
  const hasUpdate = current !== null && compareVersions(latest, current) > 0;
  return { current, latest, hasUpdate, releaseUrl };
}

/** Open a release URL in the user's default browser (system browser, not the app webview). */
export async function openReleaseUrl(url: string): Promise<void> {
  if (isTauri) {
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(url);
  } else {
    window.open(url, "_blank", "noopener,noreferrer");
  }
}

export type LinuxPackageKind = "deb" | "rpm" | "pacman";

/**
 * Which package manager (if any) owns this install. Native packages route
 * through the privileged package updater; null routes through the Tauri
 * updater (AppImage/macOS/Windows) or the browser fallback outside Tauri.
 */
export async function getLinuxPackageKind(): Promise<LinuxPackageKind | null> {
  if (!isTauri) return null;
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    const kind = await invoke<string | null>("get_linux_package_kind");
    return kind === "deb" || kind === "rpm" || kind === "pacman" ? kind : null;
  } catch {
    return null;
  }
}

/**
 * Install the update and relaunch. Never called outside Tauri (the browser
 * build only ever offers the "Download" fallback).
 *
 * For native packages, `apply_linux_package_update` resolves the asset itself from
 * the pinned repo's latest release — it never takes a URL from here, since
 * that would let webview JS point a root-privileged install at an arbitrary
 * GitHub-hosted asset.
 */
export async function installUpdate(packageKind: LinuxPackageKind | null): Promise<void> {
  if (!isTauri) throw new Error("Not running in Tauri");

  if (packageKind) {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("apply_linux_package_update");
  } else {
    const { check } = await import("@tauri-apps/plugin-updater");
    const update = await check();
    if (!update) throw new Error("No update found.");
    await update.downloadAndInstall();
  }

  const { relaunch } = await import("@tauri-apps/plugin-process");
  await relaunch();
}
