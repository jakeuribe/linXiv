import type { ReactNode } from "react";
import { Button } from "./button";

interface EmptyStateProps {
  icon: ReactNode;
  title: string;
  description: string;
  actionLabel?: string;
  onAction?: () => void;
}

export function EmptyState({ icon, title, description, actionLabel, onAction }: EmptyStateProps) {
  return (
    <div
      className="flex flex-col items-center justify-center text-center h-full min-h-[400px]"
      style={{ padding: "70px 20px" }}
    >
      <div
        className="flex items-center justify-center bg-surface2 border border-border text-ink3"
        style={{ width: 64, height: 64, borderRadius: 18, fontSize: 28 }}
      >
        {icon}
      </div>
      <h2 className="font-display text-text font-semibold" style={{ fontSize: 20, marginTop: 18 }}>
        {title}
      </h2>
      <p
        className="text-muted"
        style={{ fontSize: 13.5, marginTop: 7, maxWidth: 380, lineHeight: 1.5 }}
      >
        {description}
      </p>
      {actionLabel && onAction && (
        <Button
          variant="primary"
          onClick={onAction}
          className="shadow-card whitespace-nowrap"
          style={{ marginTop: 18 }}
        >
          {actionLabel}
        </Button>
      )}
    </div>
  );
}
