import { useId, useRef, useState, type RefObject } from "react";
import { AppearanceSection } from "../components/settings/AppearanceSection";
import { ApiKeysSection } from "../components/settings/ApiKeysSection";
import { StorageSection } from "../components/settings/StorageSection";
import { CrossRefSection } from "../components/settings/CrossRefSection";
import { SearchSection } from "../components/settings/SearchSection";
import { SidebarSection } from "../components/settings/SidebarSection";
import { ExportSection } from "../components/settings/ExportSection";
import { IntegrationsSection } from "../components/settings/IntegrationsSection";
import { TrashSection } from "../components/settings/TrashSection";
import { AboutSection } from "../components/settings/AboutSection";

function Divider() {
  return <div className="border-t border-border -mx-2 mb-4" />;
}

function AdvancedToggle({
  open,
  onToggle,
  contentId,
  buttonRef,
}: {
  open: boolean;
  onToggle: () => void;
  contentId: string;
  buttonRef: RefObject<HTMLButtonElement>;
}) {
  return (
    <h2>
      <button
        ref={buttonRef}
        type="button"
        onClick={onToggle}
        aria-expanded={open}
        aria-controls={contentId}
        className="w-full flex items-center justify-between px-4 py-3 mb-4 rounded-lg border border-border bg-panel"
      >
        <span className="text-sm font-semibold text-muted uppercase tracking-wide">
          Advanced
        </span>
        <span
          aria-hidden="true"
          className={`text-muted text-xs inline-block transition-transform duration-150 ${open ? "rotate-90" : "rotate-0"}`}
        >
          ▶
        </span>
      </button>
    </h2>
  );
}

export default function SettingsPage() {
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const advancedId = useId();
  const contentRef = useRef<HTMLDivElement>(null);
  const toggleRef = useRef<HTMLButtonElement>(null);

  function handleToggle() {
    const wasOpen = advancedOpen;
    setAdvancedOpen((o) => !o);
    // Prevent focus from being stranded inside a newly-hidden region.
    if (wasOpen && contentRef.current?.contains(document.activeElement)) {
      toggleRef.current?.focus();
    }
  }

  return (
    <div className="overflow-y-auto h-full bg-bg">
      <div className="mx-auto py-8 px-8 max-w-[800px]">
        <h1 className="text-xl font-bold text-text mb-6">Settings</h1>

        {/* Appearance is the primary section — intentionally left open by default */}
        <AppearanceSection />
        <Divider />
        <CrossRefSection defaultOpen={false} />
        <Divider />

        <AdvancedToggle
          open={advancedOpen}
          onToggle={handleToggle}
          contentId={advancedId}
          buttonRef={toggleRef}
        />

        {/*
          Container is always in the DOM so aria-controls resolves on first
          render. Children mount/unmount with the toggle — queries only run
          while the panel is open.
        */}
        <div id={advancedId} ref={contentRef}>
          {advancedOpen && (
            <>
              <ApiKeysSection defaultOpen={false} />
              <Divider />
              <StorageSection defaultOpen={false} />
              <Divider />
              <SearchSection defaultOpen={false} />
              <Divider />
              <SidebarSection defaultOpen={false} />
              <Divider />
              <ExportSection defaultOpen={false} />
              <Divider />
              <IntegrationsSection defaultOpen={false} />
              <Divider />
              <TrashSection defaultOpen={false} />
            </>
          )}
        </div>

        <Divider />
        <AboutSection />
      </div>
    </div>
  );
}
