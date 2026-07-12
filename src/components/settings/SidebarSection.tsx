import { useUiStore, type SidebarPageKey } from "../../stores/ui";
import { Toggle } from "../ui/toggle";
import { SettingGroup, SettingGroupLabel, SettingRow } from "./SettingRow";

const SIDEBAR_PAGE_OPTIONS: { key: SidebarPageKey; label: string; description: string }[] = [
  { key: "graph",  label: "Graph",      description: "Citation graph explorer" },
  { key: "search", label: "Search",     description: "arXiv / OpenAlex search" },
  { key: "doi",    label: "DOI Lookup", description: "Resolve papers by DOI" },
  { key: "tags",   label: "Tags",       description: "Tag browser" },
  { key: "notes",  label: "Editor (Notes)", description: "LaTeX editor (TeXbrain)" },
  { key: "shared", label: "Shared",     description: "P2P shared projects" },
];

export function SidebarSection() {
  const { sidebarPages, setSidebarPage } = useUiStore();

  return (
    <div>
      <SettingGroupLabel>Sidebar</SettingGroupLabel>
      <p className="mb-2.5 text-xs text-muted">
        Choose which pages appear in the sidebar navigation.
      </p>
      <SettingGroup>
        {SIDEBAR_PAGE_OPTIONS.map(({ key, label, description }) => (
          <SettingRow key={key} label={label} description={description}>
            <Toggle
              checked={sidebarPages[key]}
              onChange={(next) => setSidebarPage(key, next)}
              aria-label={label}
            />
          </SettingRow>
        ))}
      </SettingGroup>
    </div>
  );
}
