import type { Author } from "../types/api";

// Common surname particles that attach to the token(s) after them as part of the
// family name (e.g. "Ludwig van Beethoven" → last name "van Beethoven").
const PARTICLES = new Set([
  "van", "von", "der", "den", "de", "del", "della", "da", "di", "du",
  "la", "le", "los", "las", "st", "st.", "mac", "mc", "bin", "al", "ter", "ten",
]);

// Trailing generational/professional suffixes that belong with the last name,
// not the last name slot alone (e.g. "Smith Jr." not "Jr.").
const SUFFIXES = new Set([
  "jr", "jr.", "sr", "sr.", "ii", "iii", "iv", "v", "v.", "phd", "phd.", "md", "md.",
]);

export interface ParsedName {
  first: string;
  last: string;
}

// Strip one trailing suffix token (Jr., III, PhD, …), keeping ≥1 token. A
// stacked suffix ("Jr. III") leaves "Jr." for the caller's surname walk.
function stripTrailingSuffixes(tokens: string[]): { rest: string[]; suffix: string } {
  const last = tokens[tokens.length - 1];
  if (tokens.length > 1 && SUFFIXES.has(last.toLowerCase())) {
    return { rest: tokens.slice(0, -1), suffix: last };
  }
  return { rest: tokens, suffix: "" };
}

// Splits a display name into first/last with a small heuristic (no
// name-parsing dependency) — see authorName.test.ts for the shapes covered.
export function parseFullName(full: string): ParsedName {
  const name = full.trim();
  if (!name) return { first: "", last: "" };

  // Comma is an explicit "Last, First" marker; suffix handling is skipped,
  // text after the comma is taken as written.
  const comma = name.indexOf(",");
  if (comma !== -1) {
    const last = name.slice(0, comma).trim();
    const first = name.slice(comma + 1).trim();
    return { last, first };
  }

  let tokens = name.split(/\s+/);
  if (tokens.length === 1) return { first: "", last: tokens[0] };

  // Strip suffixes before the particle walk so they land on the last name
  // rather than leaking into the first-name slot.
  const { rest, suffix } = stripTrailingSuffixes(tokens);
  tokens = rest;
  if (tokens.length === 1) {
    return { first: "", last: suffix ? `${tokens[0]} ${suffix}` : tokens[0] };
  }

  // Absorb particles ("van", "de", …) leftward, but stop at i=1: a given
  // name colliding with a particle ("Van Morrison") must keep ≥1 first token.
  let i = tokens.length - 1;
  while (i > 1 && PARTICLES.has(tokens[i - 1].toLowerCase())) i--;
  const last = tokens.slice(i).join(" ");
  return {
    first: tokens.slice(0, i).join(" "),
    last: suffix ? `${last} ${suffix}` : last,
  };
}

export type NameSortBy = "full_name" | "first_name" | "last_name";

// The lowercased string to sort an author by, preferring the stored first/last
// fields and falling back to the heuristic parse of full_name.
export function nameSortKey(a: Author, by: NameSortBy): string {
  const full = a.full_name ?? "";
  if (by === "full_name") return full.toLowerCase();
  const parsed = parseFullName(full);
  const key = by === "first_name"
    ? (a.first_name || parsed.first)
    : (a.last_name || parsed.last);
  return (key || full).toLowerCase();
}
