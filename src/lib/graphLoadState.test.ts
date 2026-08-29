// Run: node --experimental-transform-types --test src/lib/graphLoadState.test.ts
//
// The Knowledge Graph host has two independent ways to hear that a load
// failed -- an `ok: false` reply from the guest, and its own dropped-reply
// timeout -- and they disagreed about the same event: the timeout deliberately
// left a settled graph on screen and re-flagged Refresh, while the reply
// unconditionally covered that graph with the full-bleed error card. These pin
// the single rule both now go through.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  GENERIC_LOAD_ERROR,
  NO_REPLY_ERROR,
  graphLoadOutcome,
  graphNoReplyOutcome,
} from "./graphLoadState.ts";

test("a successful load with nodes puts the host on the graph", () => {
  assert.deepEqual(graphLoadOutcome("loading", { ok: true, nodeCount: 4 }), {
    state: "ready",
    error: null,
    refreshError: null,
  });
});

test("a successful load that drew nothing is the empty library, not a failure", () => {
  // nodeCount is the PAYLOAD's node count, which no filter in the guest touches,
  // so 0 means the library itself is empty.
  assert.deepEqual(graphLoadOutcome("loading", { ok: true, nodeCount: 0 }), {
    state: "empty",
    error: null,
    refreshError: null,
  });
});

test("a successful load clears a failure the previous attempt reported", () => {
  const out = graphLoadOutcome("error", { ok: true, nodeCount: 2 });
  assert.equal(out.state, "ready");
  assert.equal(out.error, null);
  assert.equal(out.refreshError, null);
});

test("a first load that fails shows the error card, with the guest's reason", () => {
  assert.deepEqual(graphLoadOutcome("loading", { ok: false, error: "HTTP 500 for /api/graph" }), {
    state: "error",
    error: "HTTP 500 for /api/graph",
    refreshError: null,
  });
});

test("a retry that fails again stays on the error card", () => {
  // handleRetry puts the host back on "loading" first, so a second failure
  // arrives exactly like the first.
  const out = graphLoadOutcome("loading", { ok: false, error: "HTTP 500" });
  assert.equal(out.state, "error");
});

test("a reload that fails over a drawn graph leaves the graph alone", () => {
  // The canvas the user panned and zoomed to is still there and still valid --
  // fetchAndLoadGraph validates the payload before loadGraph destroys anything.
  const out = graphLoadOutcome("ready", { ok: false, error: "HTTP 503", hasGraph: true });
  assert.equal(out.state, "ready");
  assert.equal(out.error, null, "no full-bleed card, so nothing to caption");
  assert.equal(out.refreshError, "HTTP 503", "but the failure is still reported");
});

test("a reload that fails over the empty-library screen leaves that screen alone", () => {
  // "Nothing to graph yet" is the app's own last-known truth and carries its own
  // action; swapping it for a transient fetch error trades a real answer for a
  // worse one. Same set the dropped-reply timeout has always protected.
  const out = graphLoadOutcome("empty", { ok: false, error: "HTTP 503", hasGraph: true });
  assert.equal(out.state, "empty");
  assert.equal(out.refreshError, "HTTP 503");
});

test("a failure that destroyed the canvas still takes the screen", () => {
  // loadGraph destroys the outgoing cytoscape instance before building the new
  // one, so a throw from inside it reaches the host as the same `ok: false`
  // with the canvas genuinely blank. Keeping "ready" there would leave the user
  // staring at an empty rectangle with nothing to click.
  const out = graphLoadOutcome("ready", { ok: false, error: "renderer died", hasGraph: false });
  assert.equal(out.state, "error");
  assert.equal(out.error, "renderer died");
  assert.equal(out.refreshError, null);
});

test("a failure with no usable reason still says something", () => {
  // The card writes its own copy when `error` is null, but the header line has
  // nothing else to show, so it falls back rather than rendering "Refresh
  // failed: ".
  const out = graphLoadOutcome("ready", { ok: false, hasGraph: true });
  assert.equal(out.refreshError, GENERIC_LOAD_ERROR);
  assert.equal(graphLoadOutcome("ready", { ok: false, error: "   ", hasGraph: true }).refreshError,
    GENERIC_LOAD_ERROR);
  assert.equal(graphLoadOutcome("ready", { ok: false, error: { code: 500 }, hasGraph: true }).refreshError,
    GENERIC_LOAD_ERROR);
  // The card's own copy covers the null case, so it is left null there.
  assert.equal(graphLoadOutcome("loading", { ok: false, error: 500 }).error, null);
});

test("a failure reply from a guest that does not send hasGraph falls back to the screen", () => {
  assert.equal(graphLoadOutcome("ready", { ok: false, error: "HTTP 503" }).state, "ready");
  assert.equal(graphLoadOutcome("loading", { ok: false, error: "HTTP 503" }).state, "error");
});

test("a failure never reports itself twice, in two places at once", () => {
  // The error card and the header line are alternatives. An error card is
  // already up when prev is "error", so a further failure has nothing new to
  // say on the header -- and a card that is up always carries its own caption.
  for (const prev of ["loading", "ready", "empty", "error"] as const) {
    for (const hasGraph of [true, false]) {
      const out = graphLoadOutcome(prev, { ok: false, error: "HTTP 503", hasGraph });
      assert.ok(
        !(out.error !== null && out.refreshError !== null),
        `both surfaces claimed from "${prev}" (hasGraph: ${hasGraph})`
      );
      assert.ok(out.state === "error" || out.error === null,
        `stale card caption left from "${prev}" (hasGraph: ${hasGraph})`);
    }
  }
  assert.equal(graphNoReplyOutcome("error").refreshError, null);
});

test("only ok:false is a failure", () => {
  // The guest sends `ok: false` deliberately and `ok: true` on success; a reply
  // with neither is not turned into an error state.
  assert.equal(graphLoadOutcome("loading", { nodeCount: 3 }).state, "ready");
  assert.equal(graphLoadOutcome("loading", { ok: 0 as unknown, nodeCount: 3 }).state, "ready");
});

test("a dropped reply escalates only when nothing is on screen yet", () => {
  assert.deepEqual(graphNoReplyOutcome("loading"), {
    state: "error",
    error: null,
    refreshError: null,
  });
  assert.deepEqual(graphNoReplyOutcome("ready"), {
    state: "ready",
    error: null,
    refreshError: NO_REPLY_ERROR,
  });
  assert.deepEqual(graphNoReplyOutcome("empty"), {
    state: "empty",
    error: null,
    refreshError: NO_REPLY_ERROR,
  });
  assert.deepEqual(graphNoReplyOutcome("error"), {
    state: "error",
    error: null,
    refreshError: null,
  });
});

test("the two failure paths agree about the same screen", () => {
  // The whole point: an `ok: false` reply and a dropped reply must not send the
  // host to two different screens from the same starting state.
  for (const prev of ["loading", "ready", "empty", "error"] as const) {
    assert.equal(
      graphLoadOutcome(prev, { ok: false, error: NO_REPLY_ERROR, hasGraph: true }).state,
      graphNoReplyOutcome(prev).state,
      `disagreed from "${prev}"`
    );
  }
});
