import { useEffect, useRef, useState, type ReactNode } from "react";
import { useNavigate } from "react-router";
import { useQuery } from "@tanstack/react-query";
import { isTauri } from "../../api/client";
import { getSettings, updateSettings } from "../../api/settings";
import { queryClient } from "../../lib/queryClient";
import type { Settings } from "../../types/api";
import { checkForUpdates, type UpdateResult } from "../../api/updates";
import {
  ABOUT_GROUP_ID,
  asFrequency,
  isFrequency,
  isConclusiveCheck,
  isUpdateCheckDue,
  pickBanner,
  RECOMMENDED_FREQUENCY,
  shouldStampCheck,
  UPDATE_CHECK_QUERY_KEY,
  type UpdateFrequency,
} from "../../lib/updateSchedule";
import { Button } from "../ui/button";
import markUrl from "../../assets/linxiv-mark.svg";

/** When the last scheduled check ran, per install. */
const LAST_CHECK_KEY = "linxiv-last-update-check";

/** Set once the welcome banner has been shown. */
const WELCOME_KEY = "linxiv-welcome-seen";

const ONBOARDING_MESSAGE = "Check for new linXiv releases automatically?";
const WELCOME_MESSAGE =
  "Looks like you're new to linXiv; Settings has more features worth a look.";

function readStored(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch (e) {
    console.error(e);
    return null;
  }
}

function readLastCheck(): number | null {
  const raw = readStored(LAST_CHECK_KEY);
  return raw === null ? null : Number(raw);
}

function writeWelcomeSeen() {
  try {
    localStorage.setItem(WELCOME_KEY, "1");
  } catch (e) {
    console.error(e);
  }
}

/**
 * Runs at most one scheduled update check per app session, and only when the
 * user's chosen interval has elapsed. Reports a result only when there's
 * something newer than the running build. Passing `undefined` holds it back.
 */
function useScheduledUpdateCheck(settings: Settings | undefined): UpdateResult | null {
  const [available, setAvailable] = useState<UpdateResult | null>(null);
  const started = useRef(false);

  useEffect(() => {
    if (settings === undefined || started.current) return;
    // Outside the packaged app there is no installed version to compare
    // against, so the request could only ever come back inconclusive.
    if (!isTauri) return;
    if (!isUpdateCheckDue(asFrequency(settings.update_check_frequency), readLastCheck(), Date.now())) {
      return;
    }
    started.current = true;
    checkForUpdates()
      .then((result) => {
        if (!isConclusiveCheck(result)) return;
        // Shared with Settings › About, so following the banner's Install link
        // shows this answer rather than asking GitHub again.
        queryClient.setQueryData([UPDATE_CHECK_QUERY_KEY], result);
        if (shouldStampCheck(result)) {
          try {
            localStorage.setItem(LAST_CHECK_KEY, String(Date.now()));
          } catch (e) {
            console.error(e);
          }
        }
        if (result.hasUpdate) setAvailable(result);
      })
      .catch(console.error);
  }, [settings]);

  return available;
}

/**
 * The logo mark, tinted to the current accent. The source art is a single
 * flat colour, so masking it recolours the whole mark from one file.
 */
function LogoMark() {
  return (
    <span
      aria-hidden="true"
      className="block shrink-0"
      style={{
        width: 26,
        height: 26,
        backgroundColor: "var(--color-accent)",
        WebkitMaskImage: `url(${markUrl})`,
        maskImage: `url(${markUrl})`,
        WebkitMaskRepeat: "no-repeat",
        maskRepeat: "no-repeat",
        WebkitMaskPosition: "center",
        maskPosition: "center",
        WebkitMaskSize: "contain",
        maskSize: "contain",
      }}
    />
  );
}

/** Absolutely positioned inside the shell's <main>, above the page-level
 *  floats (z-30) and below the dialog backdrop (z-40). */
function Banner({ message, children }: { message: string; children: ReactNode }) {
  return (
    <div className="lx-rise-corner absolute bottom-4 right-4 z-[35] flex max-w-[calc(100%-2rem)] flex-wrap items-center gap-3 rounded-md border border-border bg-panel shadow-card px-3.5 py-2.5">
      <LogoMark />
      <span className="text-sm text-text">{message}</span>
      {children}
    </div>
  );
}

/** Asked while no valid `update_check_frequency` is recorded. Resolving it
 *  clears the corner for the session, whether or not the answer was stored. */
function OnboardingPrompt({ onResolved }: { onResolved: () => void }) {
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);

  // Stored first, then taken as answered.
  async function answer(frequency: UpdateFrequency) {
    setBusy(true);
    setFailed(false);
    try {
      await updateSettings({ update_check_frequency: frequency });
      onResolved();
    } catch (e) {
      console.error(e);
      setFailed(true);
    } finally {
      setBusy(false);
    }
  }

  return (
    <Banner message={ONBOARDING_MESSAGE}>
      <Button
        variant="primary"
        size="sm"
        disabled={busy}
        onClick={() => answer(RECOMMENDED_FREQUENCY)}
      >
        Check {RECOMMENDED_FREQUENCY}
      </Button>
      <Button variant="muted" size="sm" disabled={busy} onClick={() => answer("never")}>
        No thanks
      </Button>
      {failed && (
        <>
          <span className="text-xs text-danger">Couldn't save — try again.</span>
          {/* Clears the corner for this session without a write; re-asks next launch. */}
          <Button variant="muted" size="sm" onClick={onResolved}>
            Later
          </Button>
        </>
      )}
    </Banner>
  );
}

/** Follows the update question on a first run. */
function WelcomeBanner({ onDismiss }: { onDismiss: () => void }) {
  // Recorded on display, not on dismissal: an ignored welcome note would
  // otherwise reappear every launch and keep the update banner unreachable.
  useEffect(writeWelcomeSeen, []);

  return (
    <Banner message={WELCOME_MESSAGE}>
      <Button variant="muted" size="sm" onClick={onDismiss}>
        Dismiss
      </Button>
    </Banner>
  );
}

export function UpdateBanner() {
  const navigate = useNavigate();
  // This observer lives for the whole app; `updateSettings` invalidates the
  // key, so it doesn't also need a staleness poll.
  const { data: settings } = useQuery({
    queryKey: ["settings"],
    queryFn: getSettings,
    staleTime: Infinity,
  });
  const [dismissed, setDismissed] = useState(false);
  const [answered, setAnswered] = useState(false);
  const [welcomeSeen, setWelcomeSeen] = useState(() => readStored(WELCOME_KEY) !== null);

  function markWelcomeSeen() {
    writeWelcomeSeen();
    setWelcomeSeen(true);
  }

  // Not asked outside the packaged app, where the schedule can't run and the
  // answer would be inert.
  const undecided =
    isTauri && settings !== undefined && !isFrequency(settings.update_check_frequency);

  const update = useScheduledUpdateCheck(settings);

  const kind =
    settings === undefined
      ? null
      : pickBanner({ undecided, answered, welcomeSeen, hasUpdate: update !== null, dismissed });

  let message = "";
  let banner: ReactNode = null;
  if (kind === "onboarding") {
    message = ONBOARDING_MESSAGE;
    banner = <OnboardingPrompt onResolved={() => setAnswered(true)} />;
  } else if (kind === "welcome") {
    message = WELCOME_MESSAGE;
    banner = <WelcomeBanner onDismiss={markWelcomeSeen} />;
  } else if (kind === "update" && update !== null && update.latest !== null) {
    message = `Version ${update.latest} is available.`;
    banner = (
      <Banner message={message}>
        <Button
          variant="primary"
          size="sm"
          onClick={() => {
            // Settings › About owns every install path (updater plugin, pkexec
            // dpkg/rpm, browser download); the banner just points at it.
            setDismissed(true);
            navigate(`/settings#${ABOUT_GROUP_ID}`);
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

  return (
    <>
      {/* Mounted for the app's lifetime; a banner appearing later writes into
          this existing live region rather than creating one. */}
      <div role="status" aria-live="polite" className="sr-only">
        {message}
      </div>
      {banner}
    </>
  );
}
