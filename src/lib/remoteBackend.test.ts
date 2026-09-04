// Run: node --experimental-transform-types --test src/lib/remoteBackend.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { defaultAfterRemove, remoteIndicatorLabel } from "./remoteBackend.ts";
import type { RemoteBackend } from "../api/client";

const b = (id: string, label = "Lab"): RemoteBackend => ({
  id,
  label,
  node_address: "linxivnodeabc",
});

test("indicator renders only when the active backend is remote", () => {
  assert.equal(remoteIndicatorLabel(null), null); // local: nothing
  assert.equal(remoteIndicatorLabel(b("b1", "Lab node")), "Lab node");
  // A blank label still shows an unmistakable indicator.
  assert.equal(remoteIndicatorLabel(b("b1", "  ")), "Remote backend");
});

test("removing the default backend falls back to local; others keep it", () => {
  assert.equal(defaultAfterRemove(b("b1"), "b1"), null);
  const keep = b("b1");
  assert.equal(defaultAfterRemove(keep, "b2"), keep);
  assert.equal(defaultAfterRemove(null, "b1"), null);
});
