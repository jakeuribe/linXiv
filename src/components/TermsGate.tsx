import { useState, type ReactNode } from "react";
import { Button } from "./ui/button";
import { Dialog } from "./ui/dialog";
import TermsAndConditionsPage from "../pages/TermsAndConditionsPage";

const STORAGE_KEY = "linxiv-terms-accepted";

export default function TermsGate({ children }: { children: ReactNode }) {
  const [accepted, setAccepted] = useState(() => localStorage.getItem(STORAGE_KEY) === "1");
  const [checked, setChecked] = useState(false);
  const [showTerms, setShowTerms] = useState(false);

  if (accepted) return <>{children}</>;

  return (
    <div
      className="flex h-full items-center justify-center p-6"
      style={{ backgroundColor: "var(--color-bg)" }}
    >
      <div
        className="w-full max-w-md rounded-lg border p-6"
        style={{ backgroundColor: "var(--color-panel)", borderColor: "var(--color-border)" }}
      >
        <h1 className="text-lg font-semibold" style={{ color: "var(--color-text)" }}>
          Welcome to linXiv
        </h1>
        <p className="mt-2 text-sm" style={{ color: "var(--color-muted)" }}>
          Before you get started, please review and accept the Terms and Conditions.
        </p>

        <label className="mt-5 flex items-start gap-2.5 text-sm cursor-pointer" style={{ color: "var(--color-text)" }}>
          <input
            type="checkbox"
            checked={checked}
            onChange={(e) => setChecked(e.target.checked)}
            className="mt-0.5 h-4 w-4 shrink-0 accent-[var(--color-accent)]"
          />
          I have read and agree to the{" "}
          <button
            type="button"
            onClick={() => setShowTerms(true)}
            className="underline underline-offset-2 hover:opacity-80"
            style={{ color: "var(--color-accent)" }}
          >
            Terms and Conditions
          </button>
          .
        </label>

        <Button
          className="mt-5 w-full"
          disabled={!checked}
          onClick={() => {
            localStorage.setItem(STORAGE_KEY, "1");
            setAccepted(true);
          }}
        >
          Continue
        </Button>
      </div>

      <Dialog open={showTerms} onClose={() => setShowTerms(false)} title="Terms and Conditions" size="2xl">
        <div className="-mx-5.5 -my-5 rounded-md" style={{ backgroundColor: "#fff" }}>
          <TermsAndConditionsPage />
        </div>
      </Dialog>
    </div>
  );
}
