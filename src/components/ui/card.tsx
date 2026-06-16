import type { HTMLAttributes } from "react";

type HeadingTag = "h1" | "h2" | "h3" | "h4" | "h5" | "h6";

interface CardProps extends HTMLAttributes<HTMLDivElement> {
  inset?: boolean;
}

export function Card({ inset = false, className = "", ...props }: CardProps) {
  return (
    <div
      className={[
        "rounded-card border border-border shadow-card p-5.5",
        inset ? "bg-surface2" : "bg-panel",
        className,
      ].join(" ")}
      {...props}
    />
  );
}

export function MonoLabel({ className = "", ...props }: HTMLAttributes<HTMLSpanElement>) {
  return (
    <span
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
