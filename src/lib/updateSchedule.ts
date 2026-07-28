/** How often the app checks GitHub for a newer release on its own. Stored in
 *  user settings under `update_check_frequency`; "never" — no background
 *  update check — is the default for anyone who never touches the setting. */
export type UpdateFrequency = "never" | "daily" | "weekly" | "monthly";

const DAY_MS = 86_400_000;

const INTERVAL_MS: Record<UpdateFrequency, number> = {
  never: 0,
  daily: DAY_MS,
  weekly: 7 * DAY_MS,
  monthly: 30 * DAY_MS,
};

export const UPDATE_FREQUENCIES: { value: UpdateFrequency; label: string }[] = [
  { value: "never", label: "Never" },
  { value: "daily", label: "Daily" },
  { value: "weekly", label: "Weekly" },
  { value: "monthly", label: "Monthly (Recommended)" },
];

/** Narrow an untyped settings value to a frequency, falling back to "never". */
export function asFrequency(value: unknown): UpdateFrequency {
  return typeof value === "string" && value in INTERVAL_MS
    ? (value as UpdateFrequency)
    : "never";
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
  const interval = INTERVAL_MS[frequency];
  if (interval === 0) return false;
  if (lastCheck === null || !Number.isFinite(lastCheck)) return true;
  const elapsed = now - lastCheck;
  return elapsed >= interval || elapsed < 0;
}
