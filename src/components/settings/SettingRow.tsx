import type { HTMLAttributes, ReactNode } from "react";
import { MonoLabel } from "../ui/card";

type DescriptionProps =
  | {
      description: ReactNode;
      /** Sets an id on the description so a control can reference it via aria-describedby. */
      descriptionId?: string;
    }
  | { description?: undefined; descriptionId?: never };

type SettingRowProps = {
  label: ReactNode;
  children?: ReactNode;
  className?: string;
} & DescriptionProps;

export function SettingRow({
  label,
  description,
  descriptionId,
  children,
  className = "",
}: SettingRowProps) {
  return (
    <div
      className={[
        "flex items-center justify-between gap-6 py-[15px]",
        "border-b border-border last:border-0",
        className,
      ]
        .filter(Boolean)
        .join(" ")}
    >
      <div className="min-w-0">
        <span className="block text-sm font-medium text-text">{label}</span>
        {description !== undefined && (
          <p id={descriptionId} className="mt-[3px] text-xs text-muted">
            {description}
          </p>
        )}
      </div>
      {children !== undefined && (
        <div className="flex shrink-0 items-center gap-2">{children}</div>
      )}
    </div>
  );
}

type HeadingTag = "h2" | "h3" | "h4" | "h5" | "h6";

interface SettingGroupLabelProps extends HTMLAttributes<HTMLElement> {
  as?: HeadingTag;
}

export function SettingGroupLabel({
  as = "h2",
  className = "",
  ...props
}: SettingGroupLabelProps) {
  return (
    <MonoLabel
      as={as}
      className={["mb-2.5 block", className].filter(Boolean).join(" ")}
      {...props}
    />
  );
}

interface SettingGroupProps extends HTMLAttributes<HTMLDivElement> {
  /** Card padding for plain content blocks (no row dividers): "18px 20px". */
  block?: boolean;
}

export function SettingGroup({
  block = false,
  className = "",
  ...props
}: SettingGroupProps) {
  return (
    <div
      className={[
        "rounded-card border border-border bg-panel shadow-card",
        block ? "px-5 py-[18px]" : "px-5 py-1",
        className,
      ]
        .filter(Boolean)
        .join(" ")}
      {...props}
    />
  );
}
