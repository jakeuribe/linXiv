import { memo } from "react";

interface DismissControlsProps {
  dismissing: boolean;
  onDismiss: () => void;
  onRequestPermanentDismiss: () => void;
  showDismiss?: boolean;
  showNeverShow?: boolean;
  className?: string;
}

export const DismissControls = memo(function DismissControls({
  dismissing,
  onDismiss,
  onRequestPermanentDismiss,
  showDismiss = true,
  showNeverShow = true,
  className = "",
}: DismissControlsProps) {
  if (!showDismiss && !showNeverShow) return null;

  return (
    <div className={`flex flex-col items-center gap-1 border-l border-border pl-3 ${className}`}>
      {showNeverShow && (
        <button
          type="button"
          aria-label="Never show this paper again"
          title="Dismiss forever"
          disabled={dismissing}
          className="text-muted hover:text-[var(--color-danger)] transition-colors disabled:opacity-50"
          onClick={onRequestPermanentDismiss}
        >
          ✕
        </button>
      )}
      {showDismiss && (
        <button
          type="button"
          aria-label="Dismiss"
          title="Dismiss this version"
          disabled={dismissing}
          className="text-xs text-muted hover:text-text transition-colors disabled:opacity-50"
          onClick={onDismiss}
        >
          Dismiss
        </button>
      )}
    </div>
  );
});