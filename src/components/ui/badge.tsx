import type { HTMLAttributes } from "react";

type BadgeSize = "sm" | "md";

interface BadgeProps extends HTMLAttributes<HTMLSpanElement> {
  color?: string;
  size?: BadgeSize;
}

const sizeStyles: Record<BadgeSize, string> = {
  sm: "px-2 py-0.5 text-xs",
  md: "px-3 py-1 text-sm",
};

export function Badge({ color, size = "sm", className = "", style, children, ...props }: BadgeProps) {
  return (
    <span
      className={[
        "inline-flex items-center justify-center leading-none rounded-full font-medium",
        "border border-[var(--color-border)] bg-[var(--color-panel)] text-[var(--color-text)]",
        sizeStyles[size],
        className,
      ].join(" ")}
      style={
        color
          ? { borderColor: color, color, backgroundColor: `${color}22`, ...style }
          : style
      }
      {...props}
    >
      {children}
    </span>
  );
}
