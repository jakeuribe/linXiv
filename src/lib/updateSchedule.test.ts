// Run: node --experimental-strip-types --test src/lib/updateSchedule.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { asFrequency, isUpdateCheckDue } from "./updateSchedule.ts";

const DAY = 86_400_000;
const NOW = 1_700_000_000_000;

test("asFrequency falls back to never for anything unrecognised", () => {
  assert.equal(asFrequency("weekly"), "weekly");
  assert.equal(asFrequency("hourly"), "never");
  assert.equal(asFrequency(undefined), "never");
  assert.equal(asFrequency(7), "never");
});

test("never is never due", () => {
  assert.equal(isUpdateCheckDue("never", null, NOW), false);
  assert.equal(isUpdateCheckDue("never", NOW - 365 * DAY, NOW), false);
});

test("due once the interval has elapsed", () => {
  assert.equal(isUpdateCheckDue("daily", NOW - DAY, NOW), true);
  assert.equal(isUpdateCheckDue("daily", NOW - DAY / 2, NOW), false);
  assert.equal(isUpdateCheckDue("weekly", NOW - 6 * DAY, NOW), false);
  assert.equal(isUpdateCheckDue("weekly", NOW - 7 * DAY, NOW), true);
  assert.equal(isUpdateCheckDue("monthly", NOW - 30 * DAY, NOW), true);
});

test("missing or nonsense last-check is due", () => {
  assert.equal(isUpdateCheckDue("daily", null, NOW), true);
  assert.equal(isUpdateCheckDue("daily", Number.NaN, NOW), true);
});

test("a future last-check (clock moved back) is due, not stuck", () => {
  assert.equal(isUpdateCheckDue("monthly", NOW + 365 * DAY, NOW), true);
});
