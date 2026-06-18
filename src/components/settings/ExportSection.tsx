import { useUiStore, type ExportFormatKey } from "../../stores/ui";
import { Toggle } from "../ui/toggle";
import { SettingGroup, SettingGroupLabel, SettingRow } from "./SettingRow";

const EXPORT_FORMAT_OPTIONS: { key: ExportFormatKey; label: string; description: string }[] = [
  { key: "lxproj",   label: ".lxproj",  description: "linXiv project archive (papers + metadata + PDFs)" },
  { key: "bibtex",   label: "BibTeX",   description: "Standard .bib citation export" },
  { key: "obsidian", label: "Obsidian", description: "Markdown notes for Obsidian vault" },
];

export function ExportSection() {
  const exportMethods = useUiStore((s) => s.exportMethods);
  const setExportMethod = useUiStore((s) => s.setExportMethod);

  return (
    <div>
      <SettingGroupLabel>Export methods</SettingGroupLabel>
      <p className="mb-2.5 text-xs text-muted">
        Choose which export formats appear in the project export dialog.
      </p>
      <SettingGroup>
        {EXPORT_FORMAT_OPTIONS.map(({ key, label, description }) => (
          <SettingRow
            key={key}
            label={label}
            description={description}
            descriptionId={`export-desc-${key}`}
          >
            <Toggle
              checked={exportMethods[key]}
              onChange={(next) => setExportMethod(key, next)}
              aria-label={`${label} export`}
              aria-describedby={`export-desc-${key}`}
            />
          </SettingRow>
        ))}
      </SettingGroup>
    </div>
  );
}
