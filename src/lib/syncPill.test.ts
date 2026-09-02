// Run: node --experimental-strip-types --test src/lib/syncPill.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { latestSyncedAt } from "./syncPill.ts";
import type { SharedSummary } from "../api/share";

const summary = (overrides: Partial<SharedSummary>): SharedSummary => ({
  share_id: "s-1",
  name: "Shared",
  paper_count: 0,
  note_count: 0,
  tag_count: 0,
  synced_at: null,
  paused: false,
  ...overrides,
});

test("latestSyncedAt picks the most recent synced_at across shares", () => {
  assert.equal(latestSyncedAt([]), null);
  assert.equal(latestSyncedAt([summary({})]), null);

  const shares = [
    summary({ share_id: "a", synced_at: "2026-09-01T10:00:00Z" }),
    summary({ share_id: "b", synced_at: null }), // pending first sync
    summary({ share_id: "c", synced_at: "2026-09-01T12:30:00Z" }),
    summary({ share_id: "d", synced_at: "not-a-date" }), // unreadable mtime
  ];
  assert.equal(latestSyncedAt(shares), "2026-09-01T12:30:00Z");
});
