import { useEffect, useState, type ReactNode } from "react";
import { useLocation, useNavigate } from "react-router";
import { AppearanceSection } from "../components/settings/AppearanceSection";
// import { ApiKeysSection } from "../components/settings/ApiKeysSection";
import { StorageSection } from "../components/settings/StorageSection";
import { CrossRefSection } from "../components/settings/CrossRefSection";
import { OpenAlexSection } from "../components/settings/OpenAlexSection";
import { OrcidBackfillSection } from "../components/settings/OrcidBackfillSection";
import { SearchSection } from "../components/settings/SearchSection";
import { FullTextSection } from "../components/settings/FullTextSection";
import { HomeFeedSection } from "../components/settings/HomeFeedSection";
import { SidebarSection } from "../components/settings/SidebarSection";
import { ExportSection } from "../components/settings/ExportSection";
import { IntegrationsSection } from "../components/settings/IntegrationsSection";
import { SharingSection } from "../components/settings/SharingSection";
import { TrashSection } from "../components/settings/TrashSection";
import { AboutSection } from "../components/settings/AboutSection";
import { ABOUT_GROUP_ID } from "../lib/updateSchedule";
import { EditorPluginSection } from "../components/settings/EditorPluginSection";
import { ShortcutsSection } from "../components/settings/ShortcutsSection";
import { VersionMonitorSection } from "../components/settings/VersionMonitorSection";

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
      <div className="flex flex-col gap-8">
        <HomeFeedSection />
        <SearchSection />
        <FullTextSection />
        <SidebarSection />
        <VersionMonitorSection />
        <ExportSection />
      </div>
    ),
  },
  {
    id: "server",
    label: "Server & data",
    icon: "▥",
    render: () => (
      <div className="flex flex-col gap-8">
        <StorageSection />
        <TrashSection />
      </div>
    ),
  },
  {
    id: "ai",
    label: "AI & sources",
    icon: "✦",
    render: () => (
      <div className="flex flex-col gap-8">
        {/* Hidden until something reads GEMINI_API_KEY / OPENAI_API_KEY — the
            form saves keys no feature consumes yet.
        <ApiKeysSection /> */}
        <CrossRefSection />
        <OpenAlexSection />
        <OrcidBackfillSection />
      </div>
    ),
  },
  {
    id: "integrations",
    label: "Integrations",
    icon: "⚇",
    render: () => (
      <div className="flex flex-col gap-8">
        <IntegrationsSection />
        <EditorPluginSection />
      </div>
    ),
  },
  {
    id: "sharing",
    label: "Sharing",
    icon: "⇄",
    render: () => <SharingSection />,
  },
  {
    id: "shortcuts",
    label: "Shortcuts",
    icon: "⌨",
    render: () => <ShortcutsSection />,
  },
  {
    id: ABOUT_GROUP_ID,
    label: "About",
    icon: "ⓘ",
    render: () => <AboutSection />,
  },
];

function groupFromHash(hash: string): string | null {
  const id = hash.slice(1);
  return GROUPS.some((g) => g.id === id) ? id : null;
}

export default function SettingsPage() {
  const navigate = useNavigate();
  const { hash, pathname, search } = useLocation();
  const [active, setActive] = useState(() => groupFromHash(hash) ?? GROUPS[0].id);
  const activeGroup = GROUPS.find((g) => g.id === active) ?? GROUPS[0];

  // Deep link: /settings#about opens that group. Seeded above so the first
  // paint is already the right panel, and repeated here for a hash arriving
  // while the page is mounted.
  useEffect(() => {
    const id = groupFromHash(hash);
    if (id !== null) setActive(id);
  }, [hash]);

  // Picking a tab by hand takes ownership of the URL, so a reload doesn't send
  // the user back to the group a stale hash names.
  function selectGroup(id: string) {
    setActive(id);
    if (hash !== "") navigate({ pathname, search }, { replace: true });
  }

  return (
    <div className="flex h-full flex-col bg-bg">
      <div className="flex-none border-b border-border px-8 pt-7 pb-[14px]">
        <h1 className="font-display text-[27px] font-semibold leading-tight tracking-[-0.015em] text-text">
          Settings
        </h1>
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
              onClick={() => selectGroup(group.id)}
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
