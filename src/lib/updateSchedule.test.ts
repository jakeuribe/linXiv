// Run: node --experimental-transform-types --test src/lib/updateSchedule.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  asFrequency,
  isConclusiveCheck,
  isUpdateCheckDue,
  pickBanner,
  shouldStampCheck,
} from "./updateSchedule.ts";

test("a check counts only when two versions were compared", () => {
  assert.equal(isConclusiveCheck({ current: "1.2.0", latest: "1.2.0" }), true);
  assert.equal(isConclusiveCheck({ current: "1.2.0", latest: "1.3.0", error: "offline" }), false);
  // Outside the packaged app the running version is unknown.
  assert.equal(isConclusiveCheck({ current: null, latest: "1.3.0" }), false);
  // A 404 from releases/latest — no releases, or a renamed/private repo.
  assert.equal(isConclusiveCheck({ current: "1.2.0", latest: null }), false);
});

test("a pending update keeps re-checking instead of spending the interval", () => {
  assert.equal(shouldStampCheck({ current: "1.2.0", latest: "1.2.0", hasUpdate: false }), true);
  assert.equal(shouldStampCheck({ current: "1.2.0", latest: "1.3.0", hasUpdate: true }), false);
  assert.equal(shouldStampCheck({ current: null, latest: "1.3.0", hasUpdate: false }), false);
  assert.equal(
    shouldStampCheck({ current: "1.2.0", latest: "1.3.0", error: "offline", hasUpdate: false }),
    false
  );
});

const BASE = {
  undecided: false,
  answered: false,
  welcomeSeen: true,
  hasUpdate: false,
  dismissed: false,
};

test("the update question comes first, until it is answered", () => {
  assert.equal(pickBanner({ ...BASE, undecided: true }), "onboarding");
  assert.equal(pickBanner({ ...BASE, undecided: true, welcomeSeen: false }), "onboarding");
  // Answered but not yet written back to settings: don't re-ask mid-session.
  assert.equal(pickBanner({ ...BASE, undecided: true, answered: true }), null);
});

test("the welcome note follows, and outranks an update", () => {
  assert.equal(pickBanner({ ...BASE, welcomeSeen: false }), "welcome");
  assert.equal(pickBanner({ ...BASE, welcomeSeen: false, hasUpdate: true }), "welcome");
});

test("an available update shows once the first run is done", () => {
  assert.equal(pickBanner({ ...BASE, hasUpdate: true }), "update");
  assert.equal(pickBanner({ ...BASE, hasUpdate: true, dismissed: true }), null);
  assert.equal(pickBanner(BASE), null);
});

const DAY = 86_400_000;
const NOW = 1_700_000_000_000;

test("asFrequency falls back to never for anything unrecognised", () => {
  assert.equal(asFrequency("weekly"), "weekly");
  assert.equal(asFrequency("hourly"), "never");
  assert.equal(asFrequency(undefined), "never");
  assert.equal(asFrequency(7), "never");
});

test("inherited Object keys are not frequencies", () => {
  assert.equal(asFrequency("toString"), "never");
  assert.equal(asFrequency("constructor"), "never");
  assert.equal(asFrequency("__proto__"), "never");
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
