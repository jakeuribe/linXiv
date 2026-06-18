import { useState } from "react";
import { updateEnv } from "../../api/settings";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { SettingGroup, SettingGroupLabel, SettingRow } from "./SettingRow";

function PasswordField({
  label,
  value,
  onChange,
  onSave,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  onSave: () => void;
}) {
  const [show, setShow] = useState(false);

  return (
    <SettingRow label={label} description="Stored locally in your environment file.">
      <div className="relative">
        <Input
          type={show ? "text" : "password"}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          className="pr-16"
          style={{ width: 240 }}
          aria-label={label}
        />
        <button
          type="button"
          className="absolute right-2 top-1/2 -translate-y-1/2 text-xs text-muted hover:text-text transition-colors"
          onClick={() => setShow((s) => !s)}
          aria-label={show ? `Hide ${label}` : `Show ${label}`}
        >
          {show ? "Hide" : "Show"}
        </button>
      </div>
      <Button size="sm" onClick={onSave}>
        Save
      </Button>
    </SettingRow>
  );
}

export function ApiKeysSection() {
  const [geminiKey, setGeminiKey] = useState("");
  const [openaiKey, setOpenaiKey] = useState("");

  return (
    <div>
      <SettingGroupLabel>API keys</SettingGroupLabel>
      <SettingGroup>
        <PasswordField
          label="Gemini API Key"
          value={geminiKey}
          onChange={setGeminiKey}
          onSave={() => updateEnv("GEMINI_API_KEY", geminiKey).catch(console.error)}
        />
        <PasswordField
          label="OpenAI API Key"
          value={openaiKey}
          onChange={setOpenaiKey}
          onSave={() => updateEnv("OPENAI_API_KEY", openaiKey).catch(console.error)}
        />
      </SettingGroup>
    </div>
  );
}
