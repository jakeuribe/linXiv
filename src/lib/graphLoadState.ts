// What the Knowledge Graph host shows over the iframe, and how a `graph_loaded`
// reply moves it.
//
// The guest (public/graph/graph.js) is a bare cytoscape canvas with no state of
// its own to draw: an empty library, a dead backend and a still-running fetch
// all paint the same blank rectangle. So it reports `ok` / `nodeCount` / `error`
// and GraphPage renders the app's own Spinner / EmptyState over the (still
// full-size) frame. This module owns the one rule that decision needs and that
// GraphPage got wrong in one of its two failure paths.
//
// THE RULE: a reload that fails must not take away a graph that is already
// drawn. `fetchAndLoadGraph` validates the payload BEFORE it destroys anything
// -- a failed `/api/graph` throws out of the `Promise.all`, so `loadGraph` never
// runs and the settled canvas is still sitting there, intact, behind the host's
// overlay. Escalating to the full-bleed "Couldn't load the graph" card there
// hides a live graph, drops the framing the user had panned and zoomed to, and
// offers a Retry for something that has not actually been lost. GraphPage's
// dropped-reply timeout already knew this ("a dropped reply to a refresh leaves
// the settled graph alone and just re-flags the Refresh button") and implemented
// it; the `ok: false` reply -- the far more common failure, since a backend that
// answers 500 answers immediately -- did not, so the same event reached two
// different screens depending on which of the two noticed it first.
//
// A failure over a drawn graph is still reported, just not by replacing it: the
// state is left alone and the message comes back as `refreshError`, for the page
// header beside the Refresh button (whose dot GraphPage re-flags anyway).
//
// Whether a graph is still drawn is the guest's answer to give, not something to
// infer from the screen the host last painted: loadGraph() destroys the outgoing
// cytoscape instance before it builds the new one, so a throw from inside it
// reaches the host as the same `ok: false` while the canvas really is blank. The
// failure reply carries `hasGraph` for exactly that split. An older guest that
// does not send it falls back to the screen — the weaker answer, but never a
// worse one than having no field at all.

export type GraphLoadState = "loading" | "ready" | "empty" | "error";

/** The `graph_loaded` message, as it arrives — every field untrusted. */
export interface GraphLoadedReply {
  ok?: unknown;
  error?: unknown;
  nodeCount?: unknown;
  /**
   * Failure replies only: whether the guest still has a cytoscape instance, i.e.
   * whether there is anything on the canvas behind the host's overlay.
   */
  hasGraph?: unknown;
}

export interface GraphLoadOutcome {
  state: GraphLoadState;
  /**
   * Detail for the full-bleed error card. Null whenever `state` is not
   * `"error"`, so a stale message can never outlive the card that shows it.
   */
  error: string | null;
  /**
   * Detail for the page header: a reload that failed over a screen the user can
   * still use. Null on success and whenever the error card took the screen
   * instead — the two are alternatives, never both at once.
   */
  refreshError: string | null;
}

/** The guest failed but said nothing useful about why. */
export const GENERIC_LOAD_ERROR = "The graph data could not be fetched.";

/** No `graph_loaded` reply at all — GraphPage's REFRESH_FALLBACK_MS elapsed. */
export const NO_REPLY_ERROR = "The graph stopped responding.";

/** A screen the user can still read and act on, i.e. one worth protecting. */
function isDrawn(state: GraphLoadState): boolean {
  return state === "ready" || state === "empty";
}

function messageOf(error: unknown): string | null {
  if (typeof error !== "string") return null;
  const trimmed = error.trim();
  return trimmed ? trimmed : null;
}

/**
 * The screen a `graph_loaded` reply moves the host to, from the screen it is on.
 *
 * `ok !== false` is success (the guest only sends `ok: false` deliberately), and
 * `nodeCount === 0` means the LIBRARY is empty rather than "filtered down to
 * nothing" — the guest counts the payload, which no filter in it touches.
 */
export function graphLoadOutcome(
  prev: GraphLoadState,
  reply: GraphLoadedReply
): GraphLoadOutcome {
  if (reply.ok !== false) {
    return {
      state: reply.nodeCount === 0 ? "empty" : "ready",
      error: null,
      refreshError: null,
    };
  }
  // Only the two screens that ARE a usable last-known answer are protected.
  // "empty" counts: "Nothing to graph yet" is the app's own truth about the
  // library and carries its own "Go to Library" action, so swapping it for a
  // transient fetch error trades a real answer for a worse one. "loading" and
  // "error" do not: neither has anything behind it to keep, and leaving a
  // failure on the header beside an error card that is already up would report
  // the same failure twice, in two different voices.
  //
  // `hasGraph: false` overrides even the protected pair: the guest is telling
  // us the canvas is gone, so the error card is the only honest screen.
  if (isDrawn(prev) && reply.hasGraph !== false) {
    return {
      state: prev,
      error: null,
      refreshError: messageOf(reply.error) || GENERIC_LOAD_ERROR,
    };
  }
  return { state: "error", error: messageOf(reply.error), refreshError: null };
}

/**
 * The same rule for the timeout: no reply arrived at all, so there is no
 * `hasGraph` to read and the screen is all there is to go on. Separate entry
 * point because there is no reply to take a message out of, and because the
 * bootstrap case has never carried a detail line (the EmptyState's own copy
 * says it).
 */
export function graphNoReplyOutcome(prev: GraphLoadState): GraphLoadOutcome {
  if (isDrawn(prev)) {
    return { state: prev, error: null, refreshError: NO_REPLY_ERROR };
  }
  return { state: "error", error: null, refreshError: null };
}
