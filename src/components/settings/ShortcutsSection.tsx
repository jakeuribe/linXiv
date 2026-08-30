import { useEffect, useRef, useState } from "react";
import {
  SHORTCUTS,
  captureOverride,
  describeOverride,
  findConflict,
  hasBindableModifier,
  type Shortcut,
  type ShortcutScope,
} from "../../lib/shortcuts";
import { useShortcutsStore } from "../../stores/shortcuts";
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

/** Click to rebind: press a combo, Esc to cancel, blur to cancel. Warns
 * (and refuses to save) on a collision with another shortcut's binding. */
function ShortcutBinding({ shortcut }: { shortcut: Shortcut }) {
  const override = useShortcutsStore((s) => s.overrides[shortcut.id]);
  const setOverride = useShortcutsStore((s) => s.setOverride);
  const clearOverride = useShortcutsStore((s) => s.clearOverride);
  const [listening, setListening] = useState(false);
  const [conflict, setConflict] = useState<string | null>(null);
  const captureRef = useRef<HTMLSpanElement>(null);

  useEffect(() => {
    if (listening) captureRef.current?.focus();
  }, [listening]);

  // Element-scoped shortcuts (no `run`) aren't dispatched from this registry,
  // so there's nothing here to rebind — keep them read-only.
  if (!shortcut.run) {
    return <Keys keys={shortcut.keys} />;
  }

  if (listening) {
    return (
      <span className="flex items-center gap-2">
        <span
          ref={captureRef}
          tabIndex={0}
          role="button"
          aria-label="Press a key combination"
          className="rounded border border-dashed border-accent px-1.5 py-0.5 text-xs text-muted outline-none"
          onBlur={() => {
            setListening(false);
            setConflict(null);
          }}
          onKeyDown={(e) => {
            e.preventDefault();
            e.stopPropagation();
            if (e.key === "Escape") {
              setListening(false);
              setConflict(null);
              return;
            }
            if (["Control", "Meta", "Alt", "Shift"].includes(e.key)) return;
            if (!hasBindableModifier(e.nativeEvent)) {
              setConflict("Hold Ctrl, Cmd, or Alt with your key");
              return;
            }
            const hit = findConflict(
              shortcut.id,
              e.nativeEvent,
              useShortcutsStore.getState().overrides
            );
            if (hit) {
              setConflict(`Conflicts with "${hit.description}"`);
              return;
            }
            setOverride(shortcut.id, captureOverride(e.nativeEvent));
            setListening(false);
            setConflict(null);
          }}
        >
          Press keys… (Esc to cancel)
        </span>
        {conflict && <span className="text-xs text-danger">{conflict}</span>}
      </span>
    );
  }

  return (
    <span className="flex items-center gap-2">
      <button
        type="button"
        className="rounded hover:bg-surface2/60"
        onClick={() => setListening(true)}
        title="Click to rebind"
      >
        <Keys keys={override ? describeOverride(override) : shortcut.keys} />
      </button>
      {override && (
        <button
          type="button"
          className="text-xs text-muted underline hover:text-text"
          onClick={() => clearOverride(shortcut.id)}
        >
          Reset
        </button>
      )}
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
                  <ShortcutBinding shortcut={s} />
                </SettingRow>
              ))}
            </SettingGroup>
          </div>
        );
      })}
    </div>
  );
}
