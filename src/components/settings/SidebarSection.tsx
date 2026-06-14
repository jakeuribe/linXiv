import { useUiStore, type SidebarPageKey } from "../../stores/ui";
import { Section } from "./Section";

const SIDEBAR_PAGE_OPTIONS: { key: SidebarPageKey; label: string; description: string }[] = [
  { key: "graph",  label: "Graph",      description: "Citation graph explorer" },
  { key: "search", label: "Search",     description: "arXiv / OpenAlex search" },
  { key: "doi",    label: "DOI Lookup", description: "Resolve papers by DOI" },
  { key: "tags",   label: "Tags",       description: "Tag browser" },
  { key: "notes",  label: "Editor (Notes)", description: "LaTeX editor (TeXbrain)" },
];

export function SidebarSection({ defaultOpen = true }: { defaultOpen?: boolean } = {}) {
  const { sidebarPages, setSidebarPage } = useUiStore();

  return (
    <Section title="Sidebar" defaultOpen={defaultOpen}>
      <p className="text-xs text-muted mb-4">
        Choose which pages appear in the sidebar navigation.
      </p>
      {SIDEBAR_PAGE_OPTIONS.map(({ key, label, description }) => (
        <div
          key={key}
          className="flex items-center justify-between py-3 border-b border-border last:border-0"
        >
          <div className="flex-1 min-w-0 mr-4">
            <span className="text-sm font-medium text-text">{label}</span>
            <p className="text-xs text-muted mt-0.5">{description}</p>
          </div>
          <button
            type="button"
            role="switch"
            aria-checked={sidebarPages[key]}
            aria-label={label}
            onClick={() => setSidebarPage(key, !sidebarPages[key])}
            className="relative inline-flex h-6 w-11 shrink-0 cursor-pointer rounded-full border-2 border-transparent transition-colors"
            style={{
              background: sidebarPages[key] ? "var(--color-accent)" : "var(--color-border)",
            }}
          >
            <span
              className="pointer-events-none inline-block h-5 w-5 rounded-full bg-white shadow-sm transition-transform"
              style={{ transform: sidebarPages[key] ? "translateX(20px)" : "translateX(0)" }}
            />
          </button>
        </div>
      ))}
    </Section>
  );
}
