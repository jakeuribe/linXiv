/** Human-readable message for a caught unknown value. */
export function errText(err: unknown, fallback = "Unknown error"): string {
  return err instanceof Error ? err.message : fallback;
}
