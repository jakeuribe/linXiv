// Run: node --experimental-strip-types --test src/lib/pdfPosition.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  parsePdfPosition,
  pdfPositionStorageKey,
  readPdfPosition,
  writePdfPosition,
} from "./pdfPosition.ts";

test("PDF positions are scoped to the paper and version", () => {
  assert.equal(
    pdfPositionStorageKey("arxiv/2604.00068", 2),
    "linxiv-pdf-position:arxiv%2F2604.00068:v2",
  );
});

test("parsePdfPosition validates and normalizes stored positions", () => {
  assert.deepEqual(parsePdfPosition('{"page":12,"offset":0.375}'), {
    page: 12,
    offset: 0.375,
  });
  assert.deepEqual(parsePdfPosition('{"page":3.9,"offset":2}'), {
    page: 3,
    offset: 1,
  });
  assert.equal(parsePdfPosition('{"page":0,"offset":0.5}'), null);
  assert.equal(parsePdfPosition('{"page":2,"offset":"0.5"}'), null);
  assert.equal(parsePdfPosition("not json"), null);
});

test("readPdfPosition and writePdfPosition round-trip through storage", () => {
  const values = new Map<string, string>();
  const storage = {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => {
      values.set(key, value);
    },
  };

  writePdfPosition("2604.00068", 2, { page: 7, offset: 0.42 }, storage);
  assert.deepEqual(readPdfPosition("2604.00068", 2, storage), {
    page: 7,
    offset: 0.42,
  });
  assert.equal(readPdfPosition("2604.00068", 1, storage), null);
});

test("storage failures are non-fatal", () => {
  const storage = {
    getItem: (_key: string): string | null => {
      throw new Error("disabled");
    },
    setItem: (_key: string, _value: string): void => {
      throw new Error("disabled");
    },
  };

  assert.equal(readPdfPosition("paper", 1, storage), null);
  assert.doesNotThrow(() =>
    writePdfPosition("paper", 1, { page: 2, offset: 0.5 }, storage),
  );
});
