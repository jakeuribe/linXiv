import { useEffect, useRef, useState, type ReactNode } from "react";
import { useNavigate } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { getSettings, updateSettings } from "../../api/settings";
import type { Settings } from "../../types/api";
import { checkForUpdates, type UpdateResult } from "../../api/updates";
import { asFrequency, isUpdateCheckDue } from "../../lib/updateSchedule";
import { Button } from "../ui/button";

/** Per-install, not per-profile — a timestamp isn't worth a server round-trip. */
const LAST_CHECK_KEY = "linxiv.lastUpdateCheck";

function readLastCheck(): number | null {
  const raw = localStorage.getItem(LAST_CHECK_KEY);
  return raw === null ? null : Number(raw);
}

/**
 * Runs at most one scheduled update check per app session, and only when the
 * user's chosen interval has elapsed. Returns the result only when there's
 * actually something newer to install — a silent check that finds nothing (or
 * fails) must not put anything on screen.
 */
function useScheduledUpdateCheck(settings: Settings | undefined): UpdateResult | null {
  const [available, setAvailable] = useState<UpdateResult | null>(null);
  const started = useRef(false);

  useEffect(() => {
    if (settings === undefined || started.current) return;
    if (!isUpdateCheckDue(asFrequency(settings.update_check_frequency), readLastCheck(), Date.now())) {
      return;
    }
    started.current = true;
    let cancelled = false;
    checkForUpdates().then((result) => {
      // Stamp the attempt, not the success: a failed check waits out the full
      // interval rather than retrying on every render while offline.
      localStorage.setItem(LAST_CHECK_KEY, String(Date.now()));
      if (!cancelled && result.hasUpdate) setAvailable(result);
    });
    return () => {
      cancelled = true;
    };
  }, [settings]);

  return available;
}

/** Floats over the page content so it never reflows the app shell. */
function Banner({ children }: { children: ReactNode }) {
  return (
    <div
      role="status"
      className="absolute bottom-4 right-4 z-40 flex items-center gap-3 rounded-md border border-border bg-panel shadow-card px-3.5 py-2.5"
    >
      {children}
    </div>
  );
}

/**
 * Shown until the user has answered it once. An absent
 * `update_check_frequency` is the "hasn't decided yet" signal — both answers
 * write a value, so the prompt asks exactly once. A failed write leaves the
 * setting absent and the prompt returns next launch, which beats silently
 * recording an answer that never saved.
 */
function OnboardingPrompt({ onAnswered }: { onAnswered: () => void }) {
  function answer(frequency: "monthly" | "never") {
    onAnswered();
    updateSettings({ update_check_frequency: frequency }).catch(console.error);
  }

  return (
    <Banner>
      <span className="text-sm text-text">Check for new linXiv releases automatically?</span>
      <Button variant="primary" size="sm" onClick={() => answer("monthly")}>
        Check monthly
      </Button>
      <Button variant="muted" size="sm" onClick={() => answer("never")}>
        No thanks
      </Button>
    </Banner>
  );
}

export function UpdateBanner() {
  const navigate = useNavigate();
  const { data: settings } = useQuery({ queryKey: ["settings"], queryFn: getSettings });
  const update = useScheduledUpdateCheck(settings);
  const [dismissed, setDismissed] = useState(false);
  const [answered, setAnswered] = useState(false);

  // Undecided means no schedule, so no check ran — the two banners can never
  // want the same corner at the same time.
  const undecided = settings !== undefined && settings.update_check_frequency === undefined;
  if (undecided) {
    return answered ? null : <OnboardingPrompt onAnswered={() => setAnswered(true)} />;
  }

  if (update === null || dismissed) return null;

  return (
    <Banner>
      <span className="text-sm text-text">Version {update.latest} is available.</span>
      <Button
        variant="primary"
        size="sm"
        onClick={() => {
          // Settings › About owns every install path (updater plugin, pkexec
          // dpkg/rpm, browser download); the banner just points at it.
          setDismissed(true);
          navigate("/settings#about");
        }}
      >
        Install
      </Button>
      <Button variant="muted" size="sm" onClick={() => setDismissed(true)}>
        Dismiss
      </Button>
    </Banner>
  );
}
