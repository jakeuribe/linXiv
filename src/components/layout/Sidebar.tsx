import { useEffect } from "react";
import { NavLink } from "react-router-dom";
import {
  Home,
  BookOpen,
  BookMarked,
  FolderOpen,
  Network,
  Search,
  Share2,
  Link2,
  Settings,
  Tag,
  FileCode,
  Users,
  PanelLeftClose,
  PanelLeftOpen,
  Upload,
  AlertCircle,
} from "lucide-react";
import { useUiStore, type SidebarPageKey } from "../../stores/ui";
import { useImportJobsStore } from "../../stores/importJobs";
import { Spinner } from "../ui/spinner";

interface NavItem {
  to: string;
  label: string;
  icon: React.ReactNode;
  end?: boolean;
  pageKey?: SidebarPageKey;
}

const NAV_ITEMS: NavItem[] = [
  { to: "/", label: "Home", icon: <Home size={16} />, end: true },
  { to: "/library", label: "Library", icon: <BookOpen size={16} /> },
  { to: "/projects", label: "Projects", icon: <FolderOpen size={16} /> },
  { to: "/reading", label: "Reading", icon: <BookMarked size={16} />, pageKey: "reading" },
  { to: "/shared", label: "Shared", icon: <Share2 size={16} />, pageKey: "shared" },
  { to: "/graph", label: "Graph", icon: <Network size={16} />, pageKey: "graph" },
  { to: "/search", label: "Search", icon: <Search size={16} />, pageKey: "search" },
  { to: "/doi", label: "DOI", icon: <Link2 size={16} />, pageKey: "doi" },
  { to: "/tags", label: "Tags", icon: <Tag size={16} />, pageKey: "tags" },
  { to: "/authors", label: "Authors", icon: <Users size={16} /> },
  // The editor takes the Notes slot: editor projects ARE frontmatter-flagged
  // notes (see service/editor_project.py), so the tab is governed by the
  // existing "notes" sidebar toggle (ADR 0004) and will evolve into the full
  // Notes page once note handling lands. (The old /notes stub route stays
  // reachable by URL but had no page behind it.)
  { to: "/editor", label: "Editor", icon: <FileCode size={16} />, pageKey: "notes" },
  { to: "/settings", label: "Settings", icon: <Settings size={16} /> },
];

const EXPANDED_W = 160;
const COLLAPSED_W = 48;

function ImportProgress({ collapsed }: { collapsed: boolean }) {
  const jobs = useImportJobsStore((s) => s.jobs);
  const clear = useImportJobsStore((s) => s.clear);

  const processing = jobs.filter((j) => j.status === "processing").length;
  const errors = jobs.filter((j) => j.status === "error").length;
  const done = jobs.filter((j) => j.status === "done").length;
  const allSettled = jobs.length > 0 && jobs.every((j) => j.status === "done" || j.status === "error");
  const hasErrors = errors > 0;

  // Auto-clear: 4s on success, 12s on partial failure.
  // Reads store state at fire time to avoid clearing a new batch that started during the window.
  useEffect(() => {
    if (!allSettled) return;
    const delay = hasErrors ? 12000 : 4000;
    const t = setTimeout(() => {
      const current = useImportJobsStore.getState().jobs;
      if (current.every((j) => j.status === "done" || j.status === "error")) {
        clear();
      }
    }, delay);
    return () => clearTimeout(t);
  }, [allSettled, hasErrors, clear]);

  if (!jobs.length) return null;

  return (
    <div
      className="mx-2 mb-3 rounded-md px-2 py-2 text-xs flex items-center gap-2"
      style={{
        backgroundColor: hasErrors
          ? "color-mix(in srgb, var(--color-danger) 12%, transparent)"
          : "color-mix(in srgb, var(--color-accent) 10%, transparent)",
        border: `1px solid ${hasErrors ? "var(--color-danger)" : "var(--color-accent)"}`,
        color: hasErrors ? "var(--color-danger)" : "var(--color-accent)",
        opacity: 0.9,
      }}
    >
      {!allSettled
        ? <Spinner size={11} />
        : hasErrors
        ? <AlertCircle size={11} />
        : <Upload size={11} />}
      {!collapsed && (
        <span className="truncate ml-1.5 flex-1">
          {!allSettled
            ? `Importing ${processing} file${processing !== 1 ? "s" : ""}…`
            : hasErrors
            ? `${errors} failed${done > 0 ? `, ${done} done` : ""}`
            : `${done} imported`}
        </span>
      )}
      {allSettled && (
        <button
          type="button"
          onClick={clear}
          aria-label="Dismiss"
          className="ml-auto shrink-0 opacity-60 hover:opacity-100 leading-none"
          style={{ fontSize: 14 }}
        >
          ×
        </button>
      )}
    </div>
  );
}

export function Sidebar() {
  const { sidebarCollapsed, toggleSidebar, sidebarPages } = useUiStore();
  const w = sidebarCollapsed ? COLLAPSED_W : EXPANDED_W;

  return (
    <aside
      className="app-sidebar"
      style={{
        width: w,
        minWidth: w,
        backgroundColor: "var(--color-panel)",
        borderRight: "1px solid var(--color-border)",
        transition: "width 200ms ease, min-width 200ms ease",
        display: "flex",
        flexDirection: "column",
        height: "100%",
        overflow: "hidden",
      }}
    >
      {/* Logo / collapse toggle */}
      <div
        className="flex items-center px-3 py-5 select-none"
        style={{ minHeight: 56, justifyContent: sidebarCollapsed ? "center" : "space-between" }}
      >
        {!sidebarCollapsed && (
          <span
            className="text-lg font-bold tracking-tight"
            style={{ color: "var(--color-accent)" }}
          >
            linXiv
          </span>
        )}
        <button
          type="button"
          onClick={toggleSidebar}
          title={sidebarCollapsed ? "Expand sidebar" : "Collapse sidebar"}
          style={{
            color: "var(--color-muted)",
            background: "none",
            border: "none",
            cursor: "pointer",
            padding: 4,
            borderRadius: 4,
            display: "flex",
            alignItems: "center",
          }}
          onMouseEnter={(e) => ((e.currentTarget as HTMLButtonElement).style.color = "var(--color-text)")}
          onMouseLeave={(e) => ((e.currentTarget as HTMLButtonElement).style.color = "var(--color-muted)")}
        >
          {sidebarCollapsed
            ? <PanelLeftOpen size={16} />
            : <PanelLeftClose size={16} />}
        </button>
      </div>

      {/* Navigation */}
      <nav className="flex-1 px-3 flex flex-col gap-1">
        {NAV_ITEMS.filter(({ pageKey }) => !pageKey || sidebarPages[pageKey]).map(({ to, label, icon, end }) => (
          <NavLink
            key={to}
            to={to}
            end={end}
            title={sidebarCollapsed ? label : undefined}
            style={({ isActive }) => ({
              display: "flex",
              alignItems: "center",
              justifyContent: sidebarCollapsed ? "center" : "flex-start",
              gap: 8,
              padding: sidebarCollapsed ? "8px 0" : "8px 12px",
              borderRadius: 6,
              fontSize: 13,
              fontWeight: 500,
              textDecoration: "none",
              transition: "background-color 0.15s, color 0.15s",
              backgroundColor: isActive ? "var(--color-accent)" : "transparent",
              color: isActive ? "white" : "var(--color-muted)",
            })}
            onMouseEnter={(e) => {
              const el = e.currentTarget as HTMLAnchorElement;
              if (!el.getAttribute("aria-current")) {
                el.style.color = "var(--color-text)";
              }
            }}
            onMouseLeave={(e) => {
              const el = e.currentTarget as HTMLAnchorElement;
              if (!el.getAttribute("aria-current")) {
                el.style.color = "var(--color-muted)";
              }
            }}
          >
            {icon}
            {!sidebarCollapsed && <span style={{ whiteSpace: "nowrap" }}>{label}</span>}
          </NavLink>
        ))}
      </nav>

      <ImportProgress collapsed={sidebarCollapsed} />
    </aside>
  );
}
