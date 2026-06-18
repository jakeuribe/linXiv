import type { ButtonHTMLAttributes } from "react";

interface ToggleProps
  extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "onChange"> {
  checked: boolean;
  onChange?: (next: boolean) => void;
  "aria-label": string;
}

export function Toggle({
  checked,
  onChange,
  disabled,
  className = "",
  onClick,
  ...props
}: ToggleProps) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      disabled={disabled}
      onClick={(event) => {
        onChange?.(!checked);
        onClick?.(event);
      }}
      className={[
        "relative inline-flex h-5 w-9 shrink-0 items-center rounded-full px-0.5",
        "border border-[var(--color-border)] transition-colors",
        "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--color-accent)] focus-visible:ring-offset-2 focus-visible:ring-offset-[var(--color-bg)]",
        "disabled:opacity-50 disabled:cursor-not-allowed",
        checked
          ? "bg-[var(--color-accent)]"
          : "bg-[var(--color-panel)]",
        className,
      ]
        .filter(Boolean)
        .join(" ")}
      {...props}
    >
      <span
        className={[
          "inline-block h-3.5 w-3.5 rounded-full bg-[var(--color-bg)] transition-transform",
          checked ? "translate-x-4" : "translate-x-0",
        ].join(" ")}
      />
    </button>
  );
}
