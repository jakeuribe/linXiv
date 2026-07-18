export type ThemeMode = "dark" | "light";

export type ThemeColors = {
  bg: string;
  panel: string;
  border: string;
  accent: string;
  text: string;
  muted: string;
  success: string;
  danger: string;
};

export type ColorAlphas = Partial<Record<keyof ThemeColors, number>>;

export const PRESETS = {
  Navy: {
    dark: {
      bg: "#0f0f1a",
      panel: "#1a1a2e",
      border: "#2e2e50",
      accent: "#5b8dee",
      text: "#ccccdd",
      muted: "#7777aa",
      success: "#4caf88",
      danger: "#e05c6c",
      surface2: "rgba(21,21,36,0.25)",
      ink3: "#5c5c85",
    },
    light: {
      bg: "#f0f4ff",
      panel: "#ffffff",
      border: "#c8d4f0",
      accent: "#4a7de0",
      text: "#1a1a3e",
      muted: "#6677aa",
      success: "#3d9e76",
      danger: "#d64e5d",
      surface2: "#eef2fb",
      ink3: "#8896c0",
    },
  },
  Slate: {
    dark: {
      bg: "#1a1b1e",
      panel: "#25262b",
      border: "#373a40",
      accent: "#748ffc",
      text: "#c1c2c5",
      muted: "#868e96",
      success: "#51cf66",
      danger: "#ff6b6b",
      surface2: "#1f2024",
      ink3: "#6c757d",
    },
    light: {
      bg: "#f5f5f7",
      panel: "#ffffff",
      border: "#e0e1e5",
      accent: "#5a7cf8",
      text: "#1a1b1e",
      muted: "#6b7280",
      success: "#40c057",
      danger: "#fa5252",
      surface2: "#eeeef1",
      ink3: "#9097a0",
    },
  },
  Charcoal: {
    dark: {
      bg: "#1c1c1c",
      panel: "#252525",
      border: "#333333",
      accent: "#e8912d",
      text: "#d4d4d4",
      muted: "#888888",
      success: "#6abf69",
      danger: "#e57373",
      surface2: "#1f1f1f",
      ink3: "#6b6b6b",
    },
    light: {
      bg: "#f6f6f6",
      panel: "#ffffff",
      border: "#dedede",
      accent: "#d4811f",
      text: "#1c1c1c",
      muted: "#666666",
      success: "#57a85a",
      danger: "#cc5252",
      surface2: "#efefef",
      ink3: "#8c8c8c",
    },
  },
  Forest: {
    dark: {
      bg: "#0d1b12",
      panel: "#162318",
      border: "#243d2c",
      accent: "#4caf88",
      text: "#c8d8cc",
      muted: "#6b8f72",
      success: "#81c784",
      danger: "#ef9a9a",
      surface2: "#101d14",
      ink3: "#53705a",
    },
    light: {
      bg: "#f0f5f2",
      panel: "#ffffff",
      border: "#c4d9cc",
      accent: "#3a9a72",
      text: "#0d1b12",
      muted: "#527a5a",
      success: "#5aad5c",
      danger: "#e06060",
      surface2: "#e8efea",
      ink3: "#739579",
    },
  },
  Ember: {
    dark: {
      bg: "#1a1009",
      panel: "#261a0e",
      border: "#3d2b18",
      accent: "#e8912d",
      text: "#ddd0c4",
      muted: "#a0897a",
      success: "#a5d6a7",
      danger: "#ef5350",
      surface2: "#1f1409",
      ink3: "#7d6a5c",
    },
    light: {
      bg: "#fdf5ee",
      panel: "#ffffff",
      border: "#eed8be",
      accent: "#cc7a1e",
      text: "#2a1a09",
      muted: "#8a6a50",
      success: "#7ab87c",
      danger: "#d94040",
      surface2: "#f6ece1",
      ink3: "#a88f78",
    },
  },
  Cupertino: {
    light: {
      bg: "#eef1f6",
      panel: "rgba(255,255,255,0.72)",
      border: "rgba(209,209,214,0.6)",
      accent: "#007aff",
      text: "#1c1c1e",
      muted: "#8e8e93",
      success: "#34c759",
      danger: "#ff3b30",
      surface2: "rgba(120,120,128,0.12)",
      ink3: "rgba(60,60,67,0.45)",
    },
    dark: {
      bg: "#1c1c1e",
      panel: "rgba(44,44,46,0.80)",
      border: "rgba(58,58,60,0.9)",
      accent: "#0a84ff",
      text: "#ffffff",
      muted: "#8e8e93",
      success: "#34c759",
      danger: "#ff3b30",
      surface2: "rgba(118,118,128,0.24)",
      ink3: "rgba(235,235,245,0.45)",
    },
  },
  "Reading Room": {
    light: {
      bg: "#ece4d6",
      panel: "#faf6ee",
      border: "#e2d9c7",
      accent: "#b0451f",
      text: "#221d16",
      muted: "#8a7c66",
      success: "#5c9a5e",
      danger: "#c0442a",
      surface2: "#f3ecdf",
      ink3: "#a8987f",
    },
    dark: {
      bg: "#1a140d",
      panel: "#241b11",
      border: "#3a2c1c",
      accent: "#cf5a2c",
      text: "#ece4d6",
      muted: "#9c8a72",
      success: "#81c784",
      danger: "#ef5350",
      surface2: "#1f160d",
      ink3: "#6e5c47",
    },
  },
} as const;

export type PresetName = keyof typeof PRESETS;

export const VALID_HEX = /^#[0-9a-fA-F]{6}$/;


export function hexToRgba(hex: string, alpha: number): string {
  const r = parseInt(hex.slice(1, 3), 16);
  const g = parseInt(hex.slice(3, 5), 16);
  const b = parseInt(hex.slice(5, 7), 16);
  return `rgba(${r},${g},${b},${alpha.toFixed(2)})`;
}

export function getColors(
  preset: PresetName,
  mode: ThemeMode,
  overrides: Partial<ThemeColors> = {},
  overrideAlphas: ColorAlphas = {}
): ThemeColors {
  const { surface2: _surface2, ink3: _ink3, ...rest } = PRESETS[preset][mode];
  const base = { ...rest } as Record<keyof ThemeColors, string>;
  for (const k of Object.keys(overrides) as Array<keyof ThemeColors>) {
    const hex = overrides[k];
    if (hex && VALID_HEX.test(hex)) {
      const alpha = overrideAlphas[k] ?? 100;
      base[k] = alpha < 100 ? hexToRgba(hex, alpha / 100) : hex;
    }
  }
  return base as ThemeColors;
}

export function applyTheme(
  preset: PresetName,
  mode: ThemeMode,
  overrides: Partial<ThemeColors> = {},
  overrideAlphas: ColorAlphas = {}
): void {
  const colors = getColors(preset, mode, overrides, overrideAlphas);
  const { surface2, ink3 } = PRESETS[preset][mode];
  const root = document.documentElement;
  root.setAttribute("data-theme", preset.toLowerCase());
  root.setAttribute("data-mode", mode);
  root.style.setProperty("--color-bg", colors.bg);
  root.style.setProperty("--color-panel", colors.panel);
  root.style.setProperty("--color-border", colors.border);
  root.style.setProperty("--color-accent", colors.accent);
  root.style.setProperty("--color-text", colors.text);
  root.style.setProperty("--color-muted", colors.muted);
  root.style.setProperty("--color-success", colors.success);
  root.style.setProperty("--color-danger", colors.danger);
  root.style.setProperty("--color-surface-2", surface2);
  root.style.setProperty("--color-ink-3", ink3);
}
