import * as RadixDialog from "@radix-ui/react-dialog";
import type { ReactNode } from "react";
import { X } from "lucide-react";

const SIZE_CLASSES = {
  md: "max-w-md",
  lg: "max-w-lg",
  xl: "max-w-xl",
  "2xl": "max-w-2xl",
} as const;

interface DialogProps {
  open: boolean;
  onClose: () => void;
  title: string;
  children: ReactNode;
  size?: keyof typeof SIZE_CLASSES;
  // Non-modal: no overlay/focus trap, anchored to the right so the page (e.g.
  // the PDF pane) stays visible and interactive; outside clicks don't dismiss.
  // (Tab still wraps inside the content: Radix's FocusScope loops regardless
  // of modality.)
  modal?: boolean;
  // Radix Content's onOpenAutoFocus, for callers that must steer (or
  // suppress) the mount-time focus — e.g. keep a PDF text selection alive.
  onOpenAutoFocus?: (e: Event) => void;
}

export function Dialog({
  open,
  onClose,
  title,
  children,
  size = "md",
  modal = true,
  onOpenAutoFocus,
}: DialogProps) {
  const maxW = SIZE_CLASSES[size];
  return (
    <RadixDialog.Root open={open} onOpenChange={(o) => { if (!o) onClose(); }} modal={modal}>
      <RadixDialog.Portal>
        {modal ? (
          <RadixDialog.Overlay
            className="fixed inset-0 z-40 backdrop-blur-sm animate-in fade-in"
            style={{ backgroundColor: "rgba(0,0,0,0.45)" }}
          />
        ) : (
          // Invisible click shield: outside clicks must not reach page
          // controls (navigating away would silently discard the dialog's
          // state). A pane the page wants to keep interactive (the PDF)
          // raises itself above z-40 while the dialog is open.
          <div className="fixed inset-0 z-40" aria-hidden />
        )}
        <RadixDialog.Content
          onInteractOutside={modal ? undefined : (e) => e.preventDefault()}
          onOpenAutoFocus={onOpenAutoFocus}
          className={`lx-rise fixed top-1/2 z-50 flex max-h-[calc(100vh-3rem)] w-[calc(100%-2rem)] ${maxW} -translate-y-1/2 flex-col overflow-hidden ${
            modal ? "left-1/2 -translate-x-1/2" : "right-4"
          }`}
          style={{
            backgroundColor: "var(--color-panel)",
            border: "1px solid var(--color-border)",
            color: "var(--color-text)",
            borderRadius: "16px",
            boxShadow: "0 24px 70px rgba(0,0,0,0.30)",
          }}
        >
          <div
            className="flex items-center justify-between px-5.5 py-4.5"
            style={{ borderBottom: "1px solid var(--color-border)" }}
          >
            <RadixDialog.Title
              className="text-base font-semibold"
              style={{ color: "var(--color-text)" }}
            >
              {title}
            </RadixDialog.Title>
            <RadixDialog.Close asChild>
              <button
                onClick={onClose}
                className="rounded p-1 transition-colors hover:bg-[var(--color-border)]"
                style={{ color: "var(--color-muted)" }}
                aria-label="Close"
              >
                <X size={16} />
              </button>
            </RadixDialog.Close>
          </div>
          <div className="overflow-y-auto px-5.5 py-5">{children}</div>
        </RadixDialog.Content>
      </RadixDialog.Portal>
    </RadixDialog.Root>
  );
}
