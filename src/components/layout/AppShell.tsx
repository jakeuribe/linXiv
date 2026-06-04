import { Suspense, useEffect } from "react";
import { Outlet, useLocation, useNavigate } from "react-router-dom";
import { Sidebar } from "./Sidebar";
import { Spinner } from "../ui/spinner";
import GraphPage from "../../pages/GraphPage";
import { useUiStore, type SidebarPageKey } from "../../stores/ui";
import { ZOOM_STEP, DEFAULT_ZOOM } from "../../lib/zoom";
import { getSettings } from "../../api/settings";
import { useThemeStore } from "../../stores/theme";
import { VALID_HEX } from "../../lib/theme";
import type { ThemeColors, ColorAlphas } from "../../lib/theme";

const KEEP_ALIVE = ["/graph"];

const VALID_COLOR_KEYS = new Set<string>(["bg", "panel", "border", "accent", "text", "muted", "success", "danger"]);

const ROUTE_PAGE_KEY: Record<string, SidebarPageKey> = {
  "/graph":  "graph",
  "/search": "search",
  "/doi":    "doi",
  "/tags":   "tags",
  "/notes":  "notes",
};

function PageFallback() {
  return (
    <div
      className="flex-1 flex items-center justify-center"
      style={{ backgroundColor: "var(--color-bg)" }}
    >
      <Spinner size={28} />
    </div>
  );
}

export default function AppShell() {
  const { pathname } = useLocation();
  const navigate = useNavigate();
  const sidebarPages = useUiStore((s) => s.sidebarPages);
  const onKeepAlive = KEEP_ALIVE.includes(pathname);

  useEffect(() => {
    const key = ROUTE_PAGE_KEY[pathname];
    if (key && !sidebarPages[key]) {
      navigate("/", { replace: true });
    }
  }, [pathname, sidebarPages, navigate]);

  // Global zoom hotkeys: Ctrl/Cmd +/-/0 mirror the Settings zoom buttons.
  // preventDefault stops the webview's own native zoom so the two don't compound.
  // setZoom already clamps + persists; read the live value from the store so the
  // listener can stay bound once.
  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      if (!(e.ctrlKey || e.metaKey) || e.altKey) return;
      const { zoom, setZoom } = useUiStore.getState();
      if (e.key === "+" || e.key === "=") {
        e.preventDefault();
        setZoom(zoom + ZOOM_STEP);
      } else if (e.key === "-" || e.key === "_") {
        e.preventDefault();
        setZoom(zoom - ZOOM_STEP);
      } else if (e.key === "0") {
        e.preventDefault();
        setZoom(DEFAULT_ZOOM);
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  // Server is authoritative for theme overrides — restore on boot so that
  // cleared localStorage or a fresh profile picks up the saved values.
  // Note: applies after localStorage rehydration, so a brief visual re-apply
  // is possible if values differ (expected only on first boot or after cache clear).
  useEffect(() => {
    let cancelled = false;
    getSettings()
      .then((settings) => {
        if (cancelled) return;
        const rawOverrides = settings.theme_overrides ?? {};
        const rawAlphas = settings.theme_override_alphas ?? {};
        // Discard unknown keys and invalid values from a potentially corrupted store.
        const overrides = Object.fromEntries(
          Object.entries(rawOverrides).filter(([k, v]) => VALID_COLOR_KEYS.has(k) && typeof v === "string" && VALID_HEX.test(v))
        ) as Partial<ThemeColors>;
        const alphas = Object.fromEntries(
          Object.entries(rawAlphas).filter(([k, v]) => VALID_COLOR_KEYS.has(k) && typeof v === "number" && v >= 0 && v <= 100)
        ) as ColorAlphas;
        useThemeStore.getState().restoreFromSettings(overrides, alphas);
      })
      .catch(console.error);
    return () => { cancelled = true; };
  }, []);

  return (
    <div
      className="flex h-full"
      style={{ backgroundColor: "var(--color-bg)" }}
    >
      <Sidebar />
      <main
        className="flex-1 relative overflow-hidden"
        style={{ backgroundColor: "var(--color-bg)" }}
      >
        <div
          className="absolute inset-0 flex flex-col"
          style={{ display: pathname === "/graph" ? "flex" : "none" }}
        >
          <GraphPage />
        </div>

        {!onKeepAlive && (
          <div className="absolute inset-0 overflow-y-auto">
            <Suspense fallback={<PageFallback />}>
              <Outlet />
            </Suspense>
          </div>
        )}
      </main>
    </div>
  );
}