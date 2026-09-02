// Run: node --experimental-transform-types --test src/api/client.test.ts
import { test, afterEach } from "node:test";
import assert from "node:assert/strict";
import {
  ApiError,
  buildInvoke,
  mapRemoteError,
  resolveBackend,
  setDefaultBackend,
  UNREACHABLE_MESSAGE,
  type RemoteBackend,
} from "./client.ts";

const lab: RemoteBackend = {
  id: "b1",
  label: "Lab node",
  node_address: "linxivnodeabc",
};

afterEach(() => setDefaultBackend(null));

test("local default: no backend set, requests address the local `api` command", () => {
  assert.equal(resolveBackend(undefined), null);
  const { cmd, args } = buildInvoke("/api/papers", undefined, null);
  assert.equal(cmd, "api");
  assert.deepEqual(args, {
    req: { method: "GET", path: "/api/papers", body: null },
  });
});

test("remote backend routes to api_remote with the same ApiRequest shape", () => {
  const { cmd, args } = buildInvoke(
    "/api/papers/x/merge",
    { method: "POST", body: JSON.stringify({ loser_source_fk: 2 }) },
    lab
  );
  assert.equal(cmd, "api_remote");
  assert.deepEqual(args, {
    backendId: "b1",
    req: {
      method: "POST",
      path: "/api/papers/x/merge",
      body: { loser_source_fk: 2 },
    },
  });
});

test("the UI-set default applies only when the caller passes no backend", () => {
  setDefaultBackend(lab);
  assert.equal(resolveBackend(undefined), lab); // default flows in
  assert.equal(resolveBackend(null), null); // explicit local wins
  const other = { ...lab, id: "b2" };
  assert.equal(resolveBackend(other), other); // explicit remote wins
  setDefaultBackend(null);
  assert.equal(resolveBackend(undefined), null);
});

test("mapRemoteError: unreachable is the one honest offline-or-not-admitted state", () => {
  const err = mapRemoteError({ kind: "unreachable", detail: "dial failed" });
  assert.ok(err instanceof ApiError);
  assert.equal(err.status, 503);
  assert.equal(err.message, UNREACHABLE_MESSAGE);
});

test("mapRemoteError: the node's error envelope keeps its status and detail", () => {
  const err = mapRemoteError({ kind: "remote", status: 404, detail: "paper not found" });
  assert.equal(err.status, 404);
  assert.equal(err.message, "paper not found");
});

test("mapRemoteError: invalid input is a 400, junk degrades to a 500", () => {
  assert.equal(mapRemoteError({ kind: "invalid", detail: "not a node address" }).status, 400);
  assert.equal(mapRemoteError({ kind: "transport", detail: "boom" }).message, "boom");
  assert.equal(mapRemoteError("garbage").status, 500);
  assert.equal(mapRemoteError(null).status, 500);
});
