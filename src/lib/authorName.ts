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

// Strip a single trailing suffix token (Jr., III, PhD, …) off the end of a
// token list, keeping at least one token. Only the trailing token is
// stripped — with a stacked suffix like "Jr. III", the leftover "Jr." stays
// in the token list and the caller's surname walk then takes IT as the last
// name ("John Smith Jr. III" → first "John Smith", last "Jr. III", surname
// lost; pinned by authorName.test.ts).
function stripTrailingSuffixes(tokens: string[]): { rest: string[]; suffix: string } {
  const last = tokens[tokens.length - 1];
  if (tokens.length > 1 && SUFFIXES.has(last.toLowerCase())) {
    return { rest: tokens.slice(0, -1), suffix: last };
  }
  return { rest: tokens, suffix: "" };
}

// Split a display name into first/given and last/family parts with a small local
// heuristic — no name-parsing dependency. Handles the common shapes:
//   "First Last", "First Middle Last", "Last, First",
//   initials ("J. R. R. Tolkien" → first "J. R. R.", last "Tolkien"),
//   particled surnames ("Ludwig van Beethoven" → last "van Beethoven"),
//   and mononyms ("Aristotle" → last "Aristotle").
// ponytail: naive token heuristic; swap for a real name parser only if users hit
// its limits (compound given names, some non-Western orderings).
export function parseFullName(full: string): ParsedName {
  const name = full.trim();
  if (!name) return { first: "", last: "" };

  // "Last, First" — the comma is an explicit family-name-first marker.
  // Suffix handling is skipped here: anything after the comma is the given
  // name as written (e.g. "Smith, John Jr." → first "John Jr.").
  const comma = name.indexOf(",");
  if (comma !== -1) {
    const last = name.slice(0, comma).trim();
    const first = name.slice(comma + 1).trim();
    return { last, first };
  }

  let tokens = name.split(/\s+/);
  if (tokens.length === 1) return { first: "", last: tokens[0] };

  // Strip trailing suffixes (Jr., III, PhD, …) so the particle walk doesn't
  // treat them as part of the surname; reattached to the last name below.
  const { rest, suffix } = stripTrailingSuffixes(tokens);
  tokens = rest;
  if (tokens.length === 1) {
    return { first: "", last: suffix ? `${tokens[0]} ${suffix}` : tokens[0] };
  }

  // Walk left from the last token, absorbing particles ("van", "de", …) so a
  // multi-word surname stays together.
  let i = tokens.length - 1;
  while (i > 0 && PARTICLES.has(tokens[i - 1].toLowerCase())) i--;
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
