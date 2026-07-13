// Run: node --experimental-strip-types --test src/lib/shareRole.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { receivedShareRole } from "./shareRole.ts";
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

test("receivedShareRole resolves the linked share's role", () => {
  const received = [
    summary({ share_id: "s-viewer", role: "viewer" }),
    summary({ share_id: "s-editor", role: "editor" }),
    summary({ share_id: "s-plain" }), // plain mirror: no role field
  ];

  assert.equal(receivedShareRole({ share_id: "s-viewer" }, received), "viewer");
  assert.equal(receivedShareRole({ share_id: "s-editor" }, received), "editor");

  // Unknown role degrades to editable (undefined), never read-only.
  assert.equal(receivedShareRole({ share_id: "s-plain" }, received), undefined);
  assert.equal(receivedShareRole({ share_id: "s-gone" }, received), undefined);
  assert.equal(receivedShareRole({ share_id: null }, received), undefined);
  assert.equal(receivedShareRole({ share_id: "s-viewer" }, undefined), undefined);
  assert.equal(receivedShareRole(undefined, received), undefined);
});
