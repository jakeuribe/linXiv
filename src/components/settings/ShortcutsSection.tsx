import { SHORTCUTS, type ShortcutScope } from "../../lib/shortcuts";
import { SettingGroup, SettingGroupLabel, SettingRow } from "./SettingRow";

const SCOPE_LABELS: Record<ShortcutScope, string> = {
  global: "Global",
  form: "Forms & dialogs",
};

const SCOPE_ORDER: ShortcutScope[] = ["global", "form"];

function Keys({ keys }: { keys: string[] }) {
  return (
    <span className="flex items-center gap-1">
      {keys.map((k, i) => (
        <kbd
          key={i}
          className="rounded border border-border bg-surface2 px-1.5 py-0.5 text-xs font-medium text-text"
        >
          {k}
        </kbd>
      ))}
    </span>
  );
}

export function ShortcutsSection() {
  return (
    <div className="flex flex-col gap-8">
      {SCOPE_ORDER.map((scope) => {
        const rows = SHORTCUTS.filter((s) => s.scope === scope);
        if (rows.length === 0) return null;
        return (
          <div key={scope}>
            <SettingGroupLabel>{SCOPE_LABELS[scope]}</SettingGroupLabel>
            <SettingGroup>
              {rows.map((s) => (
                <SettingRow key={s.id} label={s.description}>
                  <Keys keys={s.keys} />
                </SettingRow>
              ))}
            </SettingGroup>
          </div>
        );
      })}
    </div>
  );
}
