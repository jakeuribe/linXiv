/** Format an ISO date string ("2024-01-01" or "2024-01-01T...") as a short
 *  localized date. Returns the raw string on anything unparseable, and guards
 *  against JS's silent month/day rollover (e.g. month 13, day 99). */
export function formatDate(dateStr: string | null): string {
  if (!dateStr) return "";
  // Slice to 10 chars to handle ISO 8601 timestamps ("2024-01-01T...").
  const [y, m, d] = dateStr.slice(0, 10).split("-").map(Number);
  if (!Number.isFinite(y) || !Number.isFinite(m) || !Number.isFinite(d)) return dateStr;
  const date = new Date(y, m - 1, d);
  // Detect invalid rollover (e.g. month 13 or day 99): JS silently wraps them.
  if (date.getMonth() !== m - 1 || date.getDate() !== d) return dateStr;
  return date.toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}
