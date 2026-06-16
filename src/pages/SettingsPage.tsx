import { useState, type ReactNode } from "react";
import { AppearanceSection } from "../components/settings/AppearanceSection";
import { ApiKeysSection } from "../components/settings/ApiKeysSection";
import { StorageSection } from "../components/settings/StorageSection";
import { CrossRefSection } from "../components/settings/CrossRefSection";
import { OpenAlexSection } from "../components/settings/OpenAlexSection";
import { SearchSection } from "../components/settings/SearchSection";
import { SidebarSection } from "../components/settings/SidebarSection";
import { ExportSection } from "../components/settings/ExportSection";
import { IntegrationsSection } from "../components/settings/IntegrationsSection";
import { TrashSection } from "../components/settings/TrashSection";
import { AboutSection } from "../components/settings/AboutSection";
import { EditorPluginSection } from "../components/settings/EditorPluginSection";

interface SettingsGroup {
  id: string;
  label: string;
  icon: string;
  render: () => ReactNode;
}

const GROUPS: SettingsGroup[] = [
  {
    id: "appearance",
    label: "Appearance",
    icon: "◐",
    render: () => <AppearanceSection />,
  },
  {
    id: "library",
    label: "Library",
    icon: "▤",
    render: () => (
      <>
        <SearchSection />
        <SidebarSection />
        <ExportSection />
      </>
    ),
  },
  {
    id: "server",
    label: "Server & data",
    icon: "▥",
    render: () => (
      <>
        <StorageSection />
        <TrashSection />
      </>
    ),
  },
  {
    id: "ai",
    label: "AI & sources",
    icon: "✦",
    render: () => (
      <>
        <ApiKeysSection />
        <CrossRefSection />
        <OpenAlexSection />
      </>
    ),
  },
  {
    id: "integrations",
    label: "Integrations",
    icon: "⚇",
    render: () => (
      <>
        <IntegrationsSection />
        <EditorPluginSection />
      </>
    ),
  },
  {
    id: "about",
    label: "About",
    icon: "ⓘ",
    render: () => <AboutSection />,
  },
];

export default function SettingsPage() {
  const [active, setActive] = useState(GROUPS[0].id);
  const activeGroup = GROUPS.find((g) => g.id === active) ?? GROUPS[0];

  return (
    <div className="flex h-full flex-col bg-bg">
      <div className="flex-none border-b border-border px-8 pt-7 pb-[18px]">
        <h1 className="font-display text-[27px] font-semibold leading-tight tracking-[-0.015em] text-text">
          Settings
        </h1>
        <p className="mt-[7px] text-sm text-muted">
          Configure your self-hosted linXiv instance — everything stays on your machine.
        </p>
      </div>

      <div className="grid min-h-0 flex-1" style={{ gridTemplateColumns: "212px 1fr" }}>
        <nav
          role="tablist"
          aria-orientation="vertical"
          aria-label="Settings groups"
          className="flex flex-col gap-[3px] border-r border-border bg-surface2 px-3 py-4"
        >
          {GROUPS.map((group) => (
            <button
              key={group.id}
              type="button"
              role="tab"
              id={`settings-tab-${group.id}`}
              aria-controls="settings-tabpanel"
              aria-selected={active === group.id}
              onClick={() => setActive(group.id)}
              className={[
                "flex items-center gap-2.5 rounded-md px-3 py-2 text-left text-sm transition-colors",
                active === group.id
                  ? "bg-panel font-medium text-text"
                  : "text-muted hover:text-text",
              ].join(" ")}
            >
              <span aria-hidden="true" className="w-[18px] text-center text-sm">
                {group.icon}
              </span>
              {group.label}
            </button>
          ))}
        </nav>

        <div className="overflow-y-auto px-8 pb-14 pt-6">
          <div className="mx-auto flex max-w-[760px] flex-col gap-10">
            <section
              key={activeGroup.id}
              id="settings-tabpanel"
              role="tabpanel"
              aria-labelledby={`settings-tab-${activeGroup.id}`}
            >
              <h2 className="sr-only">{activeGroup.label}</h2>
              {activeGroup.render()}
            </section>
          </div>
        </div>
      </div>
    </div>
  );
}
