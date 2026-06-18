import type { HTMLAttributes } from "react";

type HeadingTag = "h1" | "h2" | "h3" | "h4" | "h5" | "h6";

interface CardProps extends HTMLAttributes<HTMLDivElement> {
  inset?: boolean;
}

export function Card({ inset = false, className = "", style, ...props }: CardProps) {
  return (
    <div
      className={[
        "border border-border shadow-card",
        inset ? "bg-surface2" : "bg-panel",
        className,
      ].join(" ")}
      style={{ borderRadius: "var(--card-radius)", padding: "var(--card-pad)", ...style }}
      {...props}
    />
  );
}

interface MonoLabelProps extends HTMLAttributes<HTMLElement> {
  as?: "span" | HeadingTag;
}

export function MonoLabel({ as: Tag = "span", className = "", ...props }: MonoLabelProps) {
  return (
    <Tag
      className={[
        "font-mono uppercase text-ink3 text-[10.5px] tracking-[0.08em]",
        className,
      ].join(" ")}
      {...props}
    />
  );
}

interface SectionTitleProps extends HTMLAttributes<HTMLHeadingElement> {
  as?: HeadingTag;
}

export function SectionTitle({ as: Tag = "h2", className = "", ...props }: SectionTitleProps) {
  return (
    <Tag
      className={["font-display text-text", className].join(" ")}
      {...props}
    />
  );
}
