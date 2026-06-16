import type { Config } from "tailwindcss";

const serifStack = ["var(--font-display)"];

export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        bg: "var(--color-bg)",
        panel: "var(--color-panel)",
        surface2: "var(--color-surface-2)",
        border: "var(--color-border)",
        accent: "var(--color-accent)",
        text: "var(--color-text)",
        muted: "var(--color-muted)",
        ink3: "var(--color-ink-3)",
        success: "var(--color-success)",
        danger: "var(--color-danger)",
      },
      spacing: {
        1.75: "0.5rem",
        2.25: "0.642857rem",
        4.5: "1.285714rem",
        5.5: "1.571429rem",
        6.5: "1.857143rem",
        7.5: "2.142857rem",
        8.5: "2.428571rem",
      },
      borderRadius: {
        card: "0.6875rem",
      },
      boxShadow: {
        card: "var(--shadow-card)",
      },
      fontFamily: {
        sans: ["Inter", "system-ui", "sans-serif"],
        serif: serifStack,
        mono: ["JetBrains Mono", "Fira Code", "monospace"],
        display: serifStack,
        metric: ["var(--font-mono)"],
      },
    },
  },
  plugins: [],
} satisfies Config;
