// Run: node --experimental-strip-types --test src/lib/authorName.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { parseFullName, nameSortKey } from "./authorName.ts";
import type { Author } from "../types/api";

const author = (overrides: Partial<Author>): Author => ({
  author_id: 1,
  full_name: null,
  first_name: null,
  last_name: null,
  orcid: null,
  ...overrides,
});

test("parseFullName covers common name shapes", () => {
  const last = (s: string) => parseFullName(s).last;
  const first = (s: string) => parseFullName(s).first;

  // First Last
  assert.equal(last("Ada Lovelace"), "Lovelace");
  assert.equal(first("Ada Lovelace"), "Ada");

  // First Middle Last
  assert.equal(last("Katherine Grace Johnson"), "Johnson");
  assert.equal(first("Katherine Grace Johnson"), "Katherine Grace");

  // Last, First
  assert.equal(last("Tolkien, J. R. R."), "Tolkien");
  assert.equal(first("Tolkien, J. R. R."), "J. R. R.");

  // Leading initials
  assert.equal(last("J. R. R. Tolkien"), "Tolkien");
  assert.equal(first("J. R. R. Tolkien"), "J. R. R.");

  // Particled surname
  assert.equal(last("Ludwig van Beethoven"), "van Beethoven");
  assert.equal(first("Ludwig van Beethoven"), "Ludwig");

  // Mononym
  assert.equal(last("Aristotle"), "Aristotle");
  assert.equal(first("Aristotle"), "");

  // Empty / whitespace
  assert.deepEqual(parseFullName("  "), { first: "", last: "" });

  // Trailing suffix
  assert.equal(last("Robert Downey Jr."), "Downey Jr.");
  assert.equal(first("Robert Downey Jr."), "Robert");

  // Stacked suffixes (only the trailing token is stripped)
  assert.equal(last("John Smith Jr. III"), "Jr. III");
  assert.equal(first("John Smith Jr. III"), "John Smith");

  // Last, First with a trailing suffix (comma form skips suffix handling)
  assert.equal(last("Smith, John Jr."), "Smith");
  assert.equal(first("Smith, John Jr."), "John Jr.");
});

test("nameSortKey falls back to parsed full_name when first/last are null", () => {
  const a = author({ full_name: "Ludwig van Beethoven" });
  assert.equal(nameSortKey(a, "full_name"), "ludwig van beethoven");
  assert.equal(nameSortKey(a, "first_name"), "ludwig");
  assert.equal(nameSortKey(a, "last_name"), "van beethoven");
});

test("nameSortKey falls back to parsed full_name when first/last are empty strings", () => {
  const a = author({
    full_name: "Ludwig van Beethoven",
    first_name: "",
    last_name: "",
  });
  assert.equal(nameSortKey(a, "first_name"), "ludwig");
  assert.equal(nameSortKey(a, "last_name"), "van beethoven");
});

test("nameSortKey prefers stored first_name/last_name over the parsed full_name", () => {
  const a = author({
    full_name: "Grace Murray Hopper",
    first_name: "Grace",
    last_name: "Hopper",
  });
  assert.equal(nameSortKey(a, "first_name"), "grace");
  assert.equal(nameSortKey(a, "last_name"), "hopper");
});
