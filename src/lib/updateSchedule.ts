/** How often the app checks GitHub for a newer release on its own. Stored in
 *  user settings under `update_check_frequency`; "never" — no background
 *  update check — is the default for anyone who never touches the setting.
 *
 *  `update_check_frequency` must stay out of the bundled
 *  `default_settings.json`: `GET /api/settings` merges defaults over
 *  overrides, and its absence is what marks a user as not yet asked. */
export type UpdateFrequency = "never" | "daily" | "weekly" | "monthly";

/** Offered as the default answer to the launch prompt. */
export const RECOMMENDED_FREQUENCY: UpdateFrequency = "weekly";

/** The settings group holding the update controls. The banner deep-links to
 *  `#${ABOUT_GROUP_ID}`, and the arrival check keys off the same hash. */
export const ABOUT_GROUP_ID = "about";

/** Cache shared by the scheduled check and Settings › About, so the deep link
 *  doesn't ask GitHub a second question and get a different answer. */
export const UPDATE_CHECK_QUERY_KEY = "update-check";

const DAY_MS = 86_400_000;

/** Keyed by the union, so a new frequency can't compile without both its
 *  interval and its dropdown entry. `monthly` is a flat 30 days. */
const FREQUENCIES: Record<UpdateFrequency, { ms: number; label: string }> = {
  never: { ms: 0, label: "Never" },
  daily: { ms: DAY_MS, label: "Daily" },
  weekly: { ms: 7 * DAY_MS, label: "Weekly" },
  monthly: { ms: 30 * DAY_MS, label: "Monthly" },
};

/** The "(Recommended)" marker is derived, so it can't drift from the answer
 *  the launch prompt actually offers. */
export const UPDATE_FREQUENCIES: { value: UpdateFrequency; label: string }[] = (
  Object.keys(FREQUENCIES) as UpdateFrequency[]
).map((value) => ({
  value,
  label:
    value === RECOMMENDED_FREQUENCY
      ? `${FREQUENCIES[value].label} (Recommended)`
      : FREQUENCIES[value].label,
}));

/** An own-property check, not `in`: settings come from a hand-editable file,
 *  and inherited names like "toString" would otherwise pass as frequencies. */
export function isFrequency(value: unknown): value is UpdateFrequency {
  return (
    typeof value === "string" &&
    Object.prototype.hasOwnProperty.call(FREQUENCIES, value)
  );
}

/** Narrow an untyped settings value to a frequency, falling back to "never". */
export function asFrequency(value: unknown): UpdateFrequency {
  return isFrequency(value) ? value : "never";
}

/**
 * Whether two versions were actually compared. A failed request, an unreadable
 * running version (outside the packaged app), or no published release to
 * compare against — GitHub answers 404 for a renamed or private repo just as
 * it does for one with no releases — all leave nothing to conclude.
 */
export function isConclusiveCheck(result: {
  error?: string;
  current: string | null;
  latest: string | null;
}): boolean {
  return result.error === undefined && result.current !== null && result.latest !== null;
}

/**
 * Whether a conclusive check spends the interval. A pending update does not:
 * it re-checks on the next launch, so dismissing the notice mutes it for the
 * session rather than for up to a month.
 */
export function shouldStampCheck(result: {
  error?: string;
  current: string | null;
  latest: string | null;
  hasUpdate: boolean;
}): boolean {
  return isConclusiveCheck(result) && !result.hasUpdate;
}

/** Which corner banner the shell shows, if any. */
export type BannerKind = "onboarding" | "welcome" | "update";

export interface BannerState {
  /** No valid `update_check_frequency` is recorded yet. */
  undecided: boolean;
  /** The update question was answered during this session. */
  answered: boolean;
  welcomeSeen: boolean;
  hasUpdate: boolean;
  dismissed: boolean;
}

/** One banner at a time, in the order a first run meets them. */
export function pickBanner(state: BannerState): BannerKind | null {
  if (state.undecided && !state.answered) return "onboarding";
  if (!state.welcomeSeen) return "welcome";
  if (state.hasUpdate && !state.dismissed) return "update";
  return null;
}

/**
 * Whether a background check is owed. A missing, unparseable, or
 * future-dated `lastCheck` counts as due — a clock change or a corrupted
 * localStorage entry must not silently disable checks forever.
 */
export function isUpdateCheckDue(
  frequency: UpdateFrequency,
  lastCheck: number | null,
  now: number
): boolean {
  const interval = FREQUENCIES[frequency].ms;
  if (interval === 0) return false;
  if (lastCheck === null || !Number.isFinite(lastCheck)) return true;
  const elapsed = now - lastCheck;
  return elapsed >= interval || elapsed < 0;
}
