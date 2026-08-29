// Run: node --experimental-transform-types --test src/lib/graphIframe.test.ts
//
// public/graph/graph.js is a plain browser script the Knowledge Graph iframe
// loads directly — it is outside the bundler, so nothing type-checks it and
// nothing catches drift between it and the `/api/*` payloads the Rust router
// emits. This runs the real file in a `vm` context behind the smallest DOM /
// cytoscape / d3 stubs its top-level wiring needs, then drives loadGraph()
// with a payload shaped exactly like `route/graph.rs` returns.
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import { activeShortcutCombos } from "./shortcuts.ts";

const GRAPH_JS = new URL("../../public/graph/graph.js", import.meta.url);

// `let`/`const` at the top level of a vm script stay script-lexical, so append
// an accessor for the internal state the assertions read.
const PROBE = `
globalThis.__probe = () => ({
  paperAuthorLabels: _paperAuthorLabels,
  visiblePaperIds: _visiblePaperIds,
  selectedIds: _selectedIds,
  simNodeById: _simNodeById,
  tagRows: _tagRows,
  projTagFilterNames: _projTagFilterNames,
  projectFilterNames: _projectFilterNames,
  nodeColors: { paper: PAPER_COLOR, author: AUTHOR_COLOR, tag: TAG_COLOR, highlight: HIGHLIGHT_COLOR },
});`;

// The shape `GET /api/graph` returns: paper ids are SOURCE_FKs (numbers),
// author nodes are keyed `author::<AUTHOR_FK>`, tag nodes `tag::<lowercased>`.
const PAYLOAD = {
  nodes: [
    { id: 1, source_id: "arxiv:2204.1", label: "Attention", type: "paper", category: "cs.LG", tags: ["ML", "nlp"], has_pdf: true, published: "2024-01-01", url: "https://arxiv.org/abs/2204.1", doi: "10.1/a", summary: "We propose a new architecture." },
    { id: 2, source_id: "arxiv:2204.2", label: "Other", type: "paper", category: "cs.CL", tags: ["nlp"], has_pdf: false, published: "2024-02-01", url: null, doi: null, summary: null },
    { id: "author::7", label: "Ada Lovelace", type: "author", author_id: 7 },
    { id: "tag::ml", label: "ML", type: "tag" },
  ],
  edges: [
    { source: 1, target: "author::7" },
    { source: 1, target: "tag::ml" },
  ],
};

function stubElement(id: string) {
  // Element-level listeners are recorded rather than dropped so a test can
  // click a panel button the way the user does (see `click()` below).
  const handlers = new Map<string, (e?: any) => void>();
  // aria-* is the only observable for "is this announced to a screen reader",
  // and the notice toggles it alongside `display`.
  const attrs = new Map<string, string>();
  // Appended children are recorded rather than dropped: the project / project
  // tag filter rows are the only DOM in graph.js built with
  // createElement+appendChild instead of an innerHTML template, so this is the
  // only way to observe what a row was actually drawn with.
  const children: any[] = [];
  let html = "";
  const el: any = {
    id,
    value: "",
    checked: id !== "filterHasPdf",
    textContent: "",
    className: "",
    title: "",
    get innerHTML() { return html; },
    // Real innerHTML wipes the subtree, and graph.js clears a row container
    // that way before rebuilding it — without the reset a test would see every
    // row the list has ever drawn.
    set innerHTML(v: string) { html = v; children.length = 0; },
    style: { display: "", cssText: "", left: "", top: "", backgroundColor: "", setProperty() {} },
    dataset: {},
    handlers,
    attrs,
    children,
    classList: { contains: () => false, add() {}, remove() {}, toggle() {} },
    addEventListener(ev: string, fn: (e?: any) => void) { handlers.set(ev, fn); },
    setAttribute(k: string, v: string) { attrs.set(k, v); },
    appendChild(child: any) { children.push(child); return child; },
    // No layout in a vm: a real element with no box reports all zeros, and
    // graph.js reads this off #right-panels to size the fit gutter. Tests that
    // care install a realistic rect (see panelRect()).
    getBoundingClientRect: () => ({ left: 0, right: 0, top: 0, bottom: 0, width: 0, height: 0 }),
  };
  return el;
}

// Records what the last-built cytoscape/d3 pair was asked to do, so the tests
// can assert on viewport calls and replay simulation/graph events.
type Recorder = {
  fits: number;
  viewports: number;
  resizes: number;
  // How many times graph.js has built a cytoscape instance — the observable
  // for "has the canvas rendered yet", which the webfont gate defers.
  cyCreated: number;
  // What cy.width()/cy.height() report. AppShell keeps the graph iframe alive
  // behind `display: none`, and graph.css sizes #cy in vw/vh, so a hidden frame
  // really does hand cytoscape a 0x0 container.
  size: { w: number; h: number };
  // What cy.elements().boundingBox() reports — the extent _fitGraph() frames
  // when nothing is filtered or switched off.
  bbox: { x1: number; y1: number; x2: number; y2: number; w: number; h: number };
  // Per-node extents, unioned when _fitGraph() narrows the frame to the nodes
  // actually drawn. Empty by default: a node with no entry is a point.
  nodeBoxes: Map<string, { x1: number; y1: number; x2: number; y2: number }>;
  // Args of the last cy.viewport({zoom, pan}) call, which is how _fitGraph()
  // frames around the fixed #right-panels column.
  lastViewport: { zoom: number; pan: { x: number; y: number } } | null;
  // How many elements the last plain cy.fit() was handed, or null for "all of
  // them" — the fallback path's half of the narrowed frame.
  lastFitLength: number | null;
  cyHandlers: Array<{ ev: string; sel: string | undefined; fn: (e: any) => void }>;
  simHandlers: Map<string, (...a: any[]) => void>;
  // node id -> the per-element style cytoscape calls a "bypass" (what
  // `ele.style({...})` writes). Kept separate from the stylesheet because a
  // bypass outranks it and survives `cy.style(newSheet)`.
  bypasses: Map<string, Record<string, unknown>>;
  // Same, keyed `<source>-><target>`, for edges.
  edgeBypasses: Map<string, Record<string, unknown>>;
  // The cytoscape node stubs of the last-built instance, by id.
  cyNodeById: Map<string, any>;
  // What the last-built simulation currently holds under each force name.
  // `charge` is the only one graph.js parameterises PER NODE (a node the
  // attribute filters excluded is pinned in place and given strength 0), so its
  // `strength` argument is real state a test has to be able to read back.
  forces: Map<string, any>;
};
let recorder: Recorder;

// cytoscape runs a delegated handler only when the tapped element matches that
// handler's selector, and `tap` alone has four (paper / author / tag / bare
// background), so the selector is part of what a test is emitting — without it
// tapping a tag would also run the paper handler.
function emitCy(ev: string, target: any, sel?: string, originalEvent: any = {}) {
  recorder.cyHandlers
    .filter((h) => h.ev === ev && h.sel === sel)
    .forEach((h) => h.fn({ target, originalEvent }));
}

// One node's extent: whatever the test pinned for it, else a point at its own
// seed position.
function nodeBox(id: string, position: { x: number; y: number }) {
  const box = recorder.nodeBoxes.get(id);
  if (box) return box;
  return { x1: position.x, y1: position.y, x2: position.x, y2: position.y };
}

// A collection's extent, cytoscape-shaped (`w`/`h` alongside the corners).
function unionBox(nodes: any[]) {
  if (nodes.length === 0) return { x1: 0, y1: 0, x2: 0, y2: 0, w: 0, h: 0 };
  const boxes = nodes.map((n: any) => n.boundingBox());
  const x1 = Math.min(...boxes.map((b: any) => b.x1));
  const y1 = Math.min(...boxes.map((b: any) => b.y1));
  const x2 = Math.max(...boxes.map((b: any) => b.x2));
  const y2 = Math.max(...boxes.map((b: any) => b.y2));
  return { x1, y1, x2, y2, w: x2 - x1, h: y2 - y1 };
}

// cytoscape stub: collections backed by the elements graph.js hands to it.
function stubCytoscape(cfg: any) {
  recorder.cyCreated++;
  // Real cytoscape collections are filterable and can report their own extent,
  // which is how _fitGraph() frames only the nodes currently DRAWN (a hidden
  // type or, under isolate, a filtered-out node is at opacity 0 and must not
  // pull the viewport out to cover it).
  const collection = (arr: any[]): any => ({
    forEach: (f: any) => arr.forEach(f),
    length: arr.length,
    filter: (f: any) => collection(arr.filter((e, i) => f(e, i, arr))),
    boundingBox: () => unionBox(arr),
  });
  recorder.bypasses = new Map();
  recorder.edgeBypasses = new Map();
  const nodes = cfg.elements
    .filter((e: any) => e.group === "nodes")
    .map((n: any) => ({
      id: () => n.data.id,
      data: (k?: string) => (k === undefined ? n.data : n.data[k]),
      position: () => n.position,
      // Screen-space position, which is what the hover inspector is placed
      // against. No renderer here, so model coordinates stand in for it.
      renderedPosition: () => n.position,
      // Filled in below, once the edge stubs exist: filterGraph derives author
      // and tag visibility purely from a node's edges, so an empty collection
      // here would make every non-paper node look filtered out.
      connectedEdges: () => collection([]),
      // Per-node extent, so a collection can report a real bounding box.
      // Tests that care install one via `recorder.nodeBoxes`; a node with none
      // contributes a zero-size box at its own position, which is what an
      // unlaid-out vm element honestly is.
      boundingBox: () => nodeBox(n.data.id, n.position),
      style(props: Record<string, unknown>) {
        const bypass = recorder.bypasses.get(n.data.id) ?? {};
        recorder.bypasses.set(n.data.id, Object.assign(bypass, props));
      },
      empty: () => false,
    }));
  const byId = new Map(nodes.map((n: any) => [n.id(), n]));
  // Exposed so a test can emit a cytoscape event carrying the SAME node object
  // graph.js is holding, rather than a hand-rolled look-alike.
  recorder.cyNodeById = byId;
  const edges = cfg.elements
    .filter((e: any) => e.group === "edges")
    .map((e: any) => {
      const key = `${e.data.source}->${e.data.target}`;
      return {
        key,
        source: () => byId.get(e.data.source),
        target: () => byId.get(e.data.target),
        data: (k?: string) => (k === undefined ? e.data : e.data[k]),
        style(props: Record<string, unknown>) {
          const bypass = recorder.edgeBypasses.get(key) ?? {};
          recorder.edgeBypasses.set(key, Object.assign(bypass, props));
        },
      };
    });
  nodes.forEach((n: any) => {
    const mine = edges.filter((e: any) => e.source().id() === n.id() || e.target().id() === n.id());
    n.connectedEdges = () => collection(mine);
  });
  recorder.cyHandlers = [];
  return {
    nodes: (sel?: string) =>
      collection(sel ? nodes.filter((n: any) => sel.includes(`"${n.data("type")}"`)) : nodes),
    edges: () => collection(edges),
    getElementById: (id: string) =>
      byId.get(id) ?? { empty: () => true, data: () => undefined, connectedEdges: () => collection([]), style() {} },
    batch: (f: any) => f(),
    on(ev: string, sel?: any, fn?: any) {
      recorder.cyHandlers.push(
        typeof sel === "function" ? { ev, sel: undefined, fn: sel } : { ev, sel, fn }
      );
    },
    elements: () => ({ boundingBox: () => recorder.bbox }),
    fit(eles?: any) { recorder.fits++; recorder.lastFitLength = eles ? eles.length : null; },
    viewport(v: any) { recorder.viewports++; recorder.lastViewport = v; },
    minZoom: () => cfg.minZoom,
    maxZoom: () => cfg.maxZoom,
    width: () => recorder.size.w,
    height: () => recorder.size.h,
    resize() { recorder.resizes++; }, destroy() {},
    zoom: () => 1,
    pan: () => ({ x: 0, y: 0 }),
    style: () => ({ update() {} }),
  };
}

// The four GET endpoints the iframe reads (bridged in src-tauri/src/protocol.rs),
// each in the envelope its route returns.
const API_RESPONSES: Record<string, unknown> = {
  "/api/graph": PAYLOAD,
  "/api/categories": { categories: ["cs.LG", "cs.CL"] },
  "/api/tags": { tags: [{ label: "ML", paper_count: 3 }, { label: "nlp", paper_count: 1 }] },
  "/api/graph/project-options": { projects: [] },
};

/**
 * @param opts.fonts a `document.fonts` stub. Omitted by default, which is the
 *   honest shape of a `vm` context — graph.js then skips the webfont gate, so
 *   every other test still drives loadGraph() synchronously.
 * @param opts.search the iframe's `window.location.search`, i.e. the query the
 *   host froze into the src (`?api=`, `?excludeSingleAuthors=`). Default "",
 *   which is a standalone load.
 * @param opts.location overrides for `window.location` — the base-resolution
 *   fallback reads protocol/hostname/origin off it.
 * @param opts.userAgent what `navigator.userAgent` reports; the custom scheme's
 *   host form differs on Windows.
 */
function loadGraphScript(
  opts: {
    fonts?: { load(spec: string): Promise<unknown> };
    search?: string;
    location?: { protocol?: string; hostname?: string; origin?: string };
    userAgent?: string;
    innerWidth?: number;
    innerHeight?: number;
  } = {}
) {
  recorder = {
    fits: 0, viewports: 0, resizes: 0, cyCreated: 0,
    size: { w: 1200, h: 800 },
    bbox: { x1: -400, y1: -300, x2: 400, y2: 300, w: 800, h: 600 },
    nodeBoxes: new Map(),
    lastViewport: null,
    lastFitLength: null,
    cyHandlers: [], simHandlers: new Map(), bypasses: new Map(), edgeBypasses: new Map(),
    cyNodeById: new Map(), forces: new Map(),
  };
  const elements = new Map<string, any>();
  // Real custom-property storage: graph.js writes the host palette onto
  // documentElement and then reads it back through getComputedStyle, so the two
  // have to be the same store or the theme path can't be observed at all.
  const cssVars = new Map<string, string>();
  const sliderDefaults: Record<string, string> = {
    centerForce: "0.05", repelForce: "180", linkDistance: "70", linkStrength: "0.3",
  };
  const document = {
    fonts: opts.fonts,
    documentElement: { style: { setProperty: (k: string, v: string) => { cssVars.set(k, v); } } },
    getElementById(id: string) {
      if (!elements.has(id)) {
        const el = stubElement(id);
        if (sliderDefaults[id]) el.value = sliderDefaults[id];
        elements.set(id, el);
      }
      return elements.get(id);
    },
    querySelectorAll: () => [],
    createElement: () => stubElement("created"),
  };

  // Every d3 force builder is chainable and its setters are no-ops here.
  const force: any = new Proxy(function () {}, { get: () => () => force, apply: () => force });
  // ...except forceManyBody and forceCollide, whose `strength` / `radius` is
  // either a flat number or a per-node accessor depending on whether a filter
  // is narrowing the layout. Swallowing them made the two indistinguishable,
  // which is exactly the difference the Repel slider and the collision force
  // were getting wrong.
  const manyBody = () => {
    const f: any = { strength(v: any) { f.__strength = v; return f; } };
    return f;
  };
  const collide = (r?: any) => {
    const f: any = { __radius: r, radius(v: any) { f.__radius = v; return f; } };
    return f;
  };
  const d3 = {
    forceSimulation: () => {
      recorder.simHandlers = new Map();
      recorder.forces = new Map();
      return {
        on(ev: string, fn: any) { recorder.simHandlers.set(ev, fn); return this; },
        // d3: `force(name, f)` is a chainable setter, `force(name)` a getter.
        force(name: string, f?: any) {
          if (f === undefined) return recorder.forces.get(name) ?? force;
          recorder.forces.set(name, f);
          return this;
        },
        stop() {},
        alpha() { return this; },
        alphaTarget() { return this; },
        restart() { return this; },
      };
    },
    forceLink: () => force, forceManyBody: manyBody,
    forceX: () => force, forceY: () => force, forceCollide: collide,
  };

  const posted: any[] = [];
  const fetched: string[] = [];
  const listeners = new Map<string, (e: any) => void>();
  const window = {
    location: {
      protocol: "http:", hostname: "localhost", origin: "http://localhost:5180",
      ...opts.location,
      search: opts.search ?? "",
    },
    addEventListener(ev: string, fn: (e: any) => void) { listeners.set(ev, fn); },
    parent: { postMessage: (m: any) => posted.push(m) },
    navigator: { userAgent: opts.userAgent ?? "node" },
    // The hover inspector clamps itself inside these; a vm window has no
    // layout, so the default of 0 means "unknown, don't clamp".
    innerWidth: opts.innerWidth ?? 0,
    innerHeight: opts.innerHeight ?? 0,
    setTimeout, clearTimeout,
  };

  const ctx: any = vm.createContext({
    window, document, d3, console,
    cytoscape: stubCytoscape,
    fetch: async (url: string) => {
      fetched.push(url);
      // Match on the route, not on a prefix: the base is a whole origin the
      // host chooses (linxiv://localhost, http://linxiv.localhost, or the
      // document's own origin), and which one it picked is the thing under
      // test.
      const at = url.indexOf("/api");
      const path = at === -1 ? url : url.slice(at).split("?")[0];
      const body = API_RESPONSES[path];
      if (!body) return { ok: false, status: 404 };
      return { ok: true, status: 200, json: async () => structuredClone(body) };
    },
    localStorage: { getItem: () => null },
    getComputedStyle: () => ({ getPropertyValue: (k: string) => cssVars.get(k) ?? "" }),
    navigator: window.navigator,
    setTimeout, clearTimeout, URLSearchParams,
  });
  vm.runInContext(fs.readFileSync(GRAPH_JS, "utf8") + PROBE, ctx, { filename: "graph.js" });
  const send = (data: unknown) =>
    listeners.get("message")!({ data, origin: window.location.origin });
  // `keydown` carries a real event object; `resize` has nothing to read.
  const fire = (ev: string, e: any = {}) => listeners.get(ev)!(e);
  return { ctx, document, posted, fetched, cssVars, send, fire, probe: () => ctx.__probe() };
}

test("a fresh load reframes once the force layout settles", () => {
  const { ctx } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});

  // The fit inside loadGraph frames the RANDOM seed positions; d3 then spreads
  // the graph far past them, so without the settle fit the user sees an
  // almost-empty canvas.
  assert.equal(recorder.fits, 1);
  recorder.simHandlers.get("end")!();
  assert.equal(recorder.fits, 2);

  // Every drag and filter change restarts the simulation, so 'end' fires again
  // and again — the reframe must stay one-shot.
  recorder.simHandlers.get("end")!();
  assert.equal(recorder.fits, 2);
});

test("grabbing a node before the layout settles cancels the reframe", () => {
  const { ctx } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});

  emitCy("grab", { id: () => "1", position: () => ({ x: 0, y: 0 }) }, "node");
  recorder.simHandlers.get("end")!();
  assert.equal(recorder.fits, 1, "must not yank the viewport out from under a drag");
});

test("an in-place reload holds the current view instead of reframing", () => {
  const { ctx } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  recorder.simHandlers.get("end")!();
  const before = recorder.fits;

  // Refresh / option toggle: GraphPage keeps the iframe alive, so the settled
  // view must survive the reload.
  ctx.loadGraph(PAYLOAD, { preserveView: true });
  recorder.simHandlers.get("end")!();
  assert.equal(recorder.fits, before);
  assert.equal(recorder.viewports, 1);
});

test("a layout that settles while the iframe is hidden defers its fit to the reveal", () => {
  const { ctx, fire } = loadGraphScript();
  // AppShell mounts GraphPage from app boot behind `display: none`, and the
  // user can also navigate away mid-settle: either way #cy is 0x0 and
  // cytoscape's getFitViewport() bails, so cy.fit() silently does nothing and
  // the graph is left at zoom 1 / pan 0,0 with the layout spread off-screen.
  recorder.size = { w: 0, h: 0 };
  ctx.loadGraph(PAYLOAD, {});
  assert.equal(recorder.fits, 0);
  recorder.simHandlers.get("end")!();
  assert.equal(recorder.fits, 0);

  // Revealing the frame resizes its viewport, which is where the skipped fit
  // gets replayed — otherwise the graph stays framed on nothing.
  recorder.size = { w: 1200, h: 800 };
  fire("resize");
  assert.equal(recorder.resizes, 1);
  assert.equal(recorder.fits, 1);

  // ...and stays one-shot: later resizes must not yank the user's viewport.
  fire("resize");
  assert.equal(recorder.fits, 1);
});

test("a resize with nothing deferred only resizes", () => {
  const { ctx, fire } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  recorder.simHandlers.get("end")!();
  const before = recorder.fits;
  fire("resize");
  assert.equal(recorder.resizes, 1);
  assert.equal(recorder.fits, before, "a window resize must not reframe a settled graph");
});

test("an empty graph never fits — there is no bounding box to frame", () => {
  const { ctx } = loadGraphScript();
  ctx.loadGraph({ nodes: [], edges: [] }, {});
  recorder.simHandlers.get("end")!();
  assert.equal(recorder.fits, 0);
});

// graph.css floats #right-panels over the canvas: `position: fixed`, 240px
// wide, 16px from the right edge. Give the stub that geometry so the fit path
// sees the same occlusion the browser does.
function panelRect(doc: any, canvasWidth: number) {
  const left = canvasWidth - 256;
  doc.getElementById("right-panels").getBoundingClientRect = () => ({
    left, right: canvasWidth - 16, top: 16, bottom: 784, width: 240, height: 768,
  });
}

test("the fit frames clear of the fixed filter panels, not across the whole canvas", () => {
  const { ctx, document } = loadGraphScript();
  panelRect(document, 1200);
  // Off-centre extent, so the centring is actually pinned rather than falling
  // out of a symmetric box.
  recorder.bbox = { x1: 0, y1: 100, x2: 800, y2: 700, w: 800, h: 600 };
  ctx.loadGraph(PAYLOAD, {});

  // cy.fit() would zoom to the full 1200px width (1.4, capped by height at
  // 1.2) and put the rightmost node at 800 * 1.2 + 120 = 1080px — 136px deep
  // under the panel column, which starts at 944.
  assert.equal(recorder.fits, 0, "the plain full-width fit must not be used here");
  const vp = recorder.lastViewport!;
  const avail = 1200 - 256;
  assert.equal(vp.zoom, Math.min((avail - 80) / 800, (800 - 80) / 600));
  // vm-realm objects are never reference-equal to host literals under
  // deepEqual, so compare the fields.
  assert.equal(vp.pan.x, 40);
  assert.equal(vp.pan.y, -32);

  // The whole extent, padded, lands left of the panels.
  assert.ok(recorder.bbox.x2 * vp.zoom + vp.pan.x <= avail - 40);
  assert.ok(recorder.bbox.x1 * vp.zoom + vp.pan.x >= 40);
});

test("the deferred reveal fit is panel-aware too", () => {
  const { ctx, document, fire } = loadGraphScript();
  panelRect(document, 1200);
  recorder.size = { w: 0, h: 0 };
  ctx.loadGraph(PAYLOAD, {});
  recorder.simHandlers.get("end")!();
  assert.equal(recorder.lastViewport, null);

  recorder.size = { w: 1200, h: 800 };
  fire("resize");
  assert.equal(recorder.fits, 0);
  assert.equal(recorder.lastViewport!.zoom, Math.min((944 - 80) / 800, (800 - 80) / 600));
});

test("a viewport too narrow to hold the panel gutter falls back to a plain fit", () => {
  const { ctx, document } = loadGraphScript();
  // Panels wider than the canvas leaves room for: framing into what is left
  // would be a negative zoom, so the plain fit is the sane degradation.
  recorder.size = { w: 300, h: 800 };
  panelRect(document, 300);
  ctx.loadGraph(PAYLOAD, {});
  assert.equal(recorder.fits, 1);
  assert.equal(recorder.lastViewport, null);
});

// ── The fit frames what is DRAWN, not what the payload holds ────────────────
// A filter and a Visibility checkbox both take nodes off the canvas without
// taking them out of the graph, so `cy.elements().boundingBox()` is the extent
// of a set the user is not looking at. Give every fixture node a real box so a
// narrowed frame is distinguishable from the full one.
const FIT_BOXES = {
  near: new Map([
    ["1", { x1: 0, y1: 0, x2: 100, y2: 100 }],
    ["2", { x1: -100, y1: -100, x2: 0, y2: 0 }],
    ["tag::ml", { x1: 0, y1: -100, x2: 100, y2: 0 }],
    ["author::7", { x1: -100, y1: 0, x2: 0, y2: 100 }],
  ]),
};
// The four boxes above, unioned.
const NEAR_EXTENT = { w: 200, h: 200, cx: 0, cy: 0 };
// What cy.elements() reports while one node sits far off on its own: the extent
// a fit that ignores the filter would frame.
const WIDE_BBOX = { x1: -100, y1: -100, x2: 2100, y2: 2100, w: 2200, h: 2200 };

// Move one node far away and report the whole graph's extent accordingly.
function exileNode(id: string) {
  recorder.nodeBoxes = new Map(FIT_BOXES.near);
  recorder.nodeBoxes.set(id, { x1: 2000, y1: 2000, x2: 2100, y2: 2100 });
  recorder.bbox = { ...WIDE_BBOX };
}

// zoom/pan _fitGraph() produces for a 200x200 extent centred on the origin in a
// 1200x800 canvas with the 256px panel column over its right edge.
function nearViewport() {
  const avail = 1200 - 256;
  return {
    zoom: Math.min((avail - 80) / NEAR_EXTENT.w, (800 - 80) / NEAR_EXTENT.h),
    panX: avail / 2,
    panY: 800 / 2,
  };
}

test('"Randomize & restart" under "Show highlighted only" reframes the visible nodes, not the ghosts', () => {
  const { ctx, document } = loadGraphScript();
  panelRect(document, 1200);
  // Paper 2 matches neither the Category box nor anything joined to it, so
  // isolate takes it to opacity 0 — while the relayout throws its position back
  // into the seed box like every other node.
  exileNode("2");
  ctx.loadGraph(PAYLOAD, {});
  recorder.simHandlers.get("end")!();

  // Set through the panel, not through filterGraph(): the relayout button ends
  // in _applyFilter(), which re-reads every control off the DOM.
  document.getElementById("filterCategory").value = "cs.LG";
  const isolate = document.getElementById("isolate-btn");
  isolate.classList = { contains: (c: string) => c === "active", add() {}, remove() {}, toggle() {} };

  document.getElementById("relayout-btn").handlers.get("click")!();
  recorder.simHandlers.get("end")!();

  const want = nearViewport();
  const vp = recorder.lastViewport!;
  assert.equal(vp.zoom, want.zoom);
  assert.equal(vp.pan.x, want.panX);
  assert.equal(vp.pan.y, want.panY);
  // The failure this replaces: framing all four nodes puts the three visible
  // ones in a tenth of the canvas.
  assert.ok(vp.zoom > Math.min((944 - 80) / WIDE_BBOX.w, (800 - 80) / WIDE_BBOX.h) * 5);
});

test("a hidden node type is left out of the frame as well", () => {
  const { ctx, document } = loadGraphScript();
  panelRect(document, 1200);
  // Visibility > Authors off is not an attribute filter — every paper still
  // matches — but the author node is at opacity 0 all the same, so the viewport
  // must not stretch out to cover it.
  exileNode("author::7");
  ctx.loadGraph(PAYLOAD, {});
  ctx.filterGraph({ showAuthors: false });
  recorder.simHandlers.get("end")!();

  const want = nearViewport();
  assert.equal(recorder.lastViewport!.zoom, want.zoom);
  assert.equal(recorder.lastViewport!.pan.x, want.panX);
});

test("an ordinary filter ghost stays inside the frame — only isolate hides it", () => {
  const { ctx, document } = loadGraphScript();
  panelRect(document, 1200);
  exileNode("2");
  ctx.loadGraph(PAYLOAD, {});
  // Same filter as the isolate test, without isolate: paper 2 is an 8% ghost,
  // which is drawn, so the frame still has to hold it.
  ctx.filterGraph({ category: "cs.LG" });
  recorder.simHandlers.get("end")!();

  assert.equal(
    recorder.lastViewport!.zoom,
    Math.min((944 - 80) / WIDE_BBOX.w, (800 - 80) / WIDE_BBOX.h)
  );
});

test("with nothing filtered or hidden the fit still frames cy.elements()", () => {
  const { ctx, document } = loadGraphScript();
  panelRect(document, 1200);
  // Node boxes that would narrow the frame if the fit consulted them anyway:
  // an unfiltered fit must stay exactly what it was, edges included.
  recorder.nodeBoxes = new Map(FIT_BOXES.near);
  ctx.loadGraph(PAYLOAD, {});
  recorder.simHandlers.get("end")!();

  assert.equal(
    recorder.lastViewport!.zoom,
    Math.min((944 - 80) / recorder.bbox.w, (800 - 80) / recorder.bbox.h)
  );
});

test("isolate with a filter matching nothing frames the whole graph rather than an empty box", () => {
  const { ctx, document } = loadGraphScript();
  panelRect(document, 1200);
  recorder.nodeBoxes = new Map(FIT_BOXES.near);
  ctx.loadGraph(PAYLOAD, {});
  // Nothing drawn at all: there is no extent to frame, and a degenerate box
  // would leave the viewport somewhere the next filter change cannot recover
  // from. The no-match notice is what explains this state.
  ctx.filterGraph({ category: "nope", isolate: true });
  recorder.simHandlers.get("end")!();

  assert.equal(
    recorder.lastViewport!.zoom,
    Math.min((944 - 80) / recorder.bbox.w, (800 - 80) / recorder.bbox.h)
  );
});

test("the plain-fit fallback is handed the drawn nodes too", () => {
  const { ctx } = loadGraphScript();
  // No panel rect: the gutter is 0, so _fitGraph degrades to cy.fit() — which
  // takes the collection to frame, so the narrowing has to survive the
  // degradation rather than only applying on the happy path.
  exileNode("2");
  ctx.loadGraph(PAYLOAD, {});
  ctx.filterGraph({ category: "cs.LG", isolate: true });
  recorder.simHandlers.get("end")!();

  assert.equal(recorder.lastFitLength, 3, "paper 2 is at opacity 0 and must not be framed");
});

test("loadGraph indexes each paper's author labels for the Author filter", () => {
  const { ctx, probe } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});

  // Author names only reach the iframe as separate nodes joined by edges, so
  // this index is the only thing the Author highlight box can match against.
  assert.deepEqual([...probe().paperAuthorLabels.get("1")], ["ada lovelace"]);
  // Tag edges share the paper→target shape and must not pollute the index.
  assert.equal(probe().paperAuthorLabels.has("tag::ml"), false);
  assert.equal(probe().paperAuthorLabels.has("2"), false);
});

test("Author highlight filter narrows to papers by that author", () => {
  const { ctx, probe } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});

  ctx.filterGraph({ authorFilter: "lovelace" });
  assert.deepEqual([...probe().visiblePaperIds], ["1"]);

  ctx.filterGraph({ authorFilter: "hinton" });
  assert.deepEqual([...probe().visiblePaperIds], []);
});

// `/api/graph` (crates/core/src/graph.rs) selects PAPER_META.PUBLISHED straight
// out of the column, so a paper with no publication date arrives carrying
// chrono's date.min — the "0001-01-01" sentinel that storage/queries/paper.rs
// names NO_PUBLISHED_DATE and that models.rs blanks on every other endpoint.
const UNDATED_PAYLOAD = {
  nodes: [
    ...PAYLOAD.nodes,
    { id: 3, source_id: "arxiv:2204.3", label: "Undated", type: "paper", category: "cs.LG", tags: [], has_pdf: false, published: "0001-01-01" },
  ],
  edges: PAYLOAD.edges,
};

test("a Date range From keeps papers with no published date", () => {
  const { ctx, probe } = loadGraphScript();
  ctx.loadGraph(UNDATED_PAYLOAD, {});

  // Compared as a plain string, the sentinel sorts below any date a user can
  // type, so every undated paper vanished off the canvas the moment a From was
  // entered — indistinguishable from one genuinely published too early.
  ctx.filterGraph({ dateFrom: "2024-01-15" });
  assert.deepEqual([...probe().visiblePaperIds].sort(), ["2", "3"]);

  // The range still filters the papers that DO have a date.
  ctx.filterGraph({ dateFrom: "2024-01-15", dateTo: "2024-01-20" });
  assert.deepEqual([...probe().visiblePaperIds].sort(), ["3"]);
});

test("no published date means the date filter does not apply, at either bound", () => {
  const { ctx, probe } = loadGraphScript();
  ctx.loadGraph(UNDATED_PAYLOAD, {});

  // The invariant the two `d.published &&` guards already encode: an unknown
  // date is not a date to compare. The sentinel is what stopped it holding.
  ctx.filterGraph({ dateTo: "2000-01-01" });
  assert.deepEqual([...probe().visiblePaperIds], ["3"]);
  ctx.filterGraph({ dateFrom: "1900-01-01", dateTo: "1901-01-01" });
  assert.deepEqual([...probe().visiblePaperIds], ["3"]);
});

test("tag filter matches a paper's tags case-insensitively", () => {
  const { ctx, probe } = loadGraphScript();
  const rows = probe().tagRows;

  // The datalist offers canonical TAG-table labels; a paper node carries the
  // raw casing from its own metadata, so the two must still match.
  rows.push({ op: "AND", tag: "ml" });
  assert.equal(ctx._evalTagFilter(["ML", "nlp"]), true);
  rows.length = 0;

  rows.push({ op: "AND", tag: "ML" });
  assert.equal(ctx._evalTagFilter(["ml"]), true);
  assert.equal(ctx._evalTagFilter(["nlp"]), false);
});

test("project tag filter matches project tags case-insensitively", () => {
  const { ctx, probe } = loadGraphScript();
  // `/api/graph/project-options` hands back each project's tags with the TAG
  // table's own casing; the filter box takes free text.
  ctx.setFilterOptions([], [], [{ id: 5, name: "Thesis", color: "#5b8dee", tags: ["ML"] }]);
  ctx.loadGraph(
    {
      nodes: [
        { id: 1, source_id: "arxiv:1", label: "In project", type: "paper", project_ids: [5] },
        { id: 2, source_id: "arxiv:2", label: "Not in project", type: "paper", project_ids: [] },
      ],
      edges: [],
    },
    {}
  );

  // TAG labels are UNIQUE COLLATE NOCASE, so every other tag match in the app
  // is case-insensitive — typing "ml" must not silently hide the project.
  ctx.filterGraph({ projTagIds: ["ml"] });
  assert.deepEqual([...probe().visiblePaperIds], ["1"]);

  ctx.filterGraph({ projTagIds: ["ML"] });
  assert.deepEqual([...probe().visiblePaperIds], ["1"]);

  ctx.filterGraph({ projTagIds: ["unrelated"] });
  assert.deepEqual([...probe().visiblePaperIds], []);
});

// ── A Paper Tags row that stands for nothing ────────────────────────────────
// The rows are free text matched WHOLE against a paper's own tag list, so a
// typo — or a tag renamed/merged/deleted elsewhere since the row was added —
// empties the canvas from a row that reads exactly like a working one. The
// Projects and Project Tags lists have marked that case since the swatch work;
// this list is the one that inherited the `unmatched` style and never set it.

/** One Paper Tags row as graph.js drew it: [op-or-spacer, label, remove]. */
function tagFilterRows(document: any) {
  return document.getElementById("tag-filter-rows").children.map((row: any) => ({
    label: row.children[1].textContent,
    unmatched: row.className.split(" ").includes("unmatched"),
    title: row.title,
  }));
}

function addTagRow(ctx: any, document: any, value: string) {
  document.getElementById("tagFilterInput").value = value;
  ctx._addTag();
}

test("a Paper Tags row matching no paper's tags is marked as such", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});

  // "ml" is on paper 1 (spelled "ML"); "mL" is the same tag, "nope" is not one.
  addTagRow(ctx, document, "mL");
  addTagRow(ctx, document, "nope");

  const rows = tagFilterRows(document);
  assert.deepEqual(rows.map((r: any) => r.label), ["mL", "nope"]);
  assert.equal(rows[0].unmatched, false);
  assert.equal(rows[1].unmatched, true);
  assert.match(rows[1].title, /No paper/);
  // The reason it has to be visible: this row alone empties the canvas.
  assert.equal(ctx._evalTagFilter(["ML", "nlp"]), false);
});

test("a Paper Tags row is not called unmatched before anything has loaded", () => {
  const { ctx, document } = loadGraphScript();
  // No payload yet, so there is no tag universe to check against — claiming the
  // row matches nothing would be a claim an empty canvas cannot support.
  addTagRow(ctx, document, "ml");
  assert.equal(tagFilterRows(document)[0].unmatched, false);
});

test("a reload repaints the tag rows against the payload it just drew", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  addTagRow(ctx, document, "nlp");
  assert.equal(tagFilterRows(document)[0].unmatched, false);

  // The tag is untagged / renamed / merged away elsewhere in the app, then
  // Refresh. Nothing else on the reload path redraws this list, so the row
  // would otherwise keep reading as a working filter over an empty canvas.
  ctx.loadGraph(
    {
      nodes: [{ id: 1, source_id: "arxiv:2204.1", label: "Attention", type: "paper", tags: ["ML"] }],
      edges: [],
    },
    { preserveView: true }
  );
  assert.equal(tagFilterRows(document)[0].unmatched, true);
});

// ── ...and the list that row is added FROM ──────────────────────────────────
// `/api/tags` is `list_tags_with_count` (crates/core/src/storage/queries/tag.rs),
// which LEFT JOINs the whole TAG table and keeps the rows that join to nothing —
// its own test pins a `paper_count: 0` row in the answer. Projects share that
// table (add_project_tags inserts into TAG) and remove_paper_tags leaves the row
// behind when a paper drops its last link, so /api/tags routinely names tags no
// paper carries. Offering those in a box that matches
// against a PAPER's own tag list is offering a filter that can only empty the
// canvas — and, since the marking above, one that draws itself as "matches
// nothing" the moment it is picked out of the list.

test("the Paper Tags dropdown only offers tags a paper on the canvas carries", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  // "queue" is a PROJECT's tag and "retired" is one whose last paper dropped it.
  // Both are still TAG rows, so both come back from /api/tags alongside the two
  // the payload's papers actually carry.
  ctx.setFilterOptions(null, ["ML", "nlp", "queue", "retired"], null);

  assert.deepEqual(datalistValues(document, "tagList"), ["ML", "nlp"]);
});

test("the dropdown narrows the offer without narrowing what a typed row can match", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  ctx.setFilterOptions(null, ["ML", "queue"], null);
  assert.deepEqual(datalistValues(document, "tagList"), ["ML"]);

  // Same split setFilterOptions makes for the reading-list marker: the OFFER is
  // narrowed, the MATCH is untouched. A row typed by hand still reaches
  // _evalTagFilter, still filters, and still marks itself when it stands for
  // nothing — none of which the datalist is allowed to decide.
  addTagRow(ctx, document, "ML");
  assert.equal(tagFilterRows(document)[0].unmatched, false);
  assert.equal(ctx._evalTagFilter(["ML", "nlp"]), true);
  addTagRow(ctx, document, "queue");
  assert.equal(tagFilterRows(document)[1].unmatched, true);
  assert.equal(ctx._evalTagFilter(["ML", "nlp"]), false);
});

test("the offer is case-folded, as every tag match in this app is", () => {
  const { ctx, document } = loadGraphScript();
  // TAG.TAG is UNIQUE COLLATE NOCASE, so /api/tags hands back the canonical
  // label while a paper node carries the raw casing from its own metadata —
  // exactly the pair _evalTagFilter folds before comparing.
  ctx.loadGraph(
    { nodes: [{ id: 1, source_id: "arxiv:1", label: "T", type: "paper", tags: ["ml"] }], edges: [] },
    {}
  );
  ctx.setFilterOptions(null, ["ML"], null);
  assert.deepEqual(datalistValues(document, "tagList"), ["ML"]);
});

test("nothing is withheld before a payload has arrived", () => {
  const { ctx, document } = loadGraphScript();
  // No canvas to check against yet. Withholding here would hide every tag in
  // the library behind a graph that has not loaded — the same rule that keeps a
  // row from being called unmatched at this point.
  ctx.setFilterOptions(null, ["ML", "queue"], null);
  assert.deepEqual(datalistValues(document, "tagList"), ["ML", "queue"]);
});

test("a reload stops offering a tag that left the graph", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  ctx.setFilterOptions(null, ["ML", "nlp"], null);
  assert.deepEqual(datalistValues(document, "tagList"), ["ML", "nlp"]);

  // "nlp" is untagged off both papers elsewhere in the app, then Refresh. The
  // TAG row outlives the link, so /api/tags will keep answering with it forever
  // — the reload path is the only place that can notice, and it is the same
  // reload that repaints the rows.
  ctx.loadGraph(
    {
      nodes: [{ id: 1, source_id: "arxiv:2204.1", label: "Attention", type: "paper", tags: ["ML"] }],
      edges: [],
    },
    { preserveView: true }
  );
  assert.deepEqual(datalistValues(document, "tagList"), ["ML"]);
});

// ── ...and the whitespace the three of them disagreed over ──────────────────
// crates/core/src/graph.rs TRIMS a tag (dropping it when nothing is left)
// before it builds the `tag::<lower>` node id, but forwards each paper's `tags`
// array raw — and nothing upstream guarantees the two agree. `POST
// /api/papers/{id}/tags` trims its body, but the archive import path does not:
// export_import.rs hands the archive's own strings to add_paper_tags, which
// stores them verbatim in PAPER_META.TAGS and creates the TAG row through
// tag_fk_for_label, which does not trim either. Folding only the CASE therefore
// split one tag three ways: the canvas drew a chip "ml", /api/tags offered
// "ml " (invisible in a dropdown, and not a NOCASE duplicate of "ml"), and
// _addTag trimmed the pick back to "ml", which matched no paper.

const PADDED_TAG_PAYLOAD = {
  nodes: [
    { id: 1, source_id: "arxiv:1", label: "Attention", type: "paper", tags: ["  ML  ", "nlp"] },
    // The node graph.rs draws for that tag: trimmed, lowercased, labelled with
    // the trimmed spelling. It is what the user sees and clicks.
    { id: "tag::ml", label: "ML", type: "tag" },
  ],
  edges: [{ source: 1, target: "tag::ml" }],
};

test("a tag the paper spelled with padding still matches the chip the canvas drew", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PADDED_TAG_PAYLOAD, {});

  // The row is what the user can type after reading the canvas.
  addTagRow(ctx, document, "ml");
  assert.equal(tagFilterRows(document)[0].unmatched, false);
  assert.equal(ctx._evalTagFilter(["  ML  ", "nlp"]), true);
});

test("the dropdown offers a padded TAG label in the form a row can hold", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PADDED_TAG_PAYLOAD, {});
  // What list_tags_with_count answers with for a tag imported from an archive.
  ctx.setFilterOptions(null, ["  ML  ", "nlp"], null);
  assert.deepEqual(datalistValues(document, "tagList"), ["ML", "nlp"]);

  // The whole point of the offered form: picking it builds a row that filters.
  addTagRow(ctx, document, datalistValues(document, "tagList")[0]);
  assert.equal(tagFilterRows(document)[0].unmatched, false);
  assert.equal(ctx._evalTagFilter(["  ML  "]), true);
});

test("a padded and a trimmed spelling of one tag are offered once", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PADDED_TAG_PAYLOAD, {});
  // TAG.TAG's UNIQUE COLLATE NOCASE stops "ml" and "ML" coexisting but not
  // "ml " and "ml", so both rows really can come back from /api/tags.
  ctx.setFilterOptions(null, ["ML ", "ml"], null);
  assert.deepEqual(datalistValues(document, "tagList"), ["ML"]);
});

test("a tag that is nothing but whitespace is no part of the canvas's universe", () => {
  const { ctx, document } = loadGraphScript();
  // graph.rs drops it rather than drawing a node for it, so neither may the
  // list that claims to name what the canvas carries.
  ctx.loadGraph(
    { nodes: [{ id: 1, source_id: "arxiv:1", label: "T", type: "paper", tags: ["ml", "   "] }], edges: [] },
    {}
  );
  ctx.setFilterOptions(null, ["ml", "   "], null);
  assert.deepEqual(datalistValues(document, "tagList"), ["ml"]);
});

test("a filter list refuses a case-variant duplicate of a row it already holds", () => {
  const { ctx, document, probe } = loadGraphScript();
  const rows = probe().projTagFilterNames;

  document.getElementById("filterProjectTag").value = "ML";
  ctx._addToFilterList("filterProjectTag", rows, ctx._renderProjTagRows);
  // Matching is case-insensitive downstream, so a second row spelled "ml"
  // would filter identically while reading as an extra condition.
  document.getElementById("filterProjectTag").value = "ml";
  ctx._addToFilterList("filterProjectTag", rows, ctx._renderProjTagRows);

  // Spread out of the vm realm: its Array has a different prototype.
  assert.deepEqual([...rows], ["ML"]);
  assert.equal(document.getElementById("filterProjectTag").value, "");
});

test("a full fetch populates the datalists with labels, not stringified objects", async () => {
  const { ctx, document, posted } = loadGraphScript();
  // `/api/tags` returns `{label, paper_count}` rows, not bare strings — feeding
  // them to the datalist unmapped renders `[object Object]` options.
  await ctx.fetchAndLoadGraph({ preserveView: false });

  assert.equal(document.getElementById("tagList").innerHTML, '<option value="ML"><option value="nlp">');
  assert.equal(document.getElementById("categoryList").innerHTML, '<option value="cs.LG"><option value="cs.CL">');
  assert.ok(posted.some((m: any) => m.type === "graph_loaded" && m.ok === true));
});

// The 8 tokens src/lib/theme.ts getColors() resolves, as GraphPage posts them.
const HOST_PALETTE = {
  bg: "#f0f5f2", panel: "#ffffff", border: "#c4d9cc", accent: "#3a9a72",
  text: "#0d1b12", muted: "#527a5a", success: "#5aad5c", danger: "#e06060",
};

test("a theme_update forwards every token the host resolved", () => {
  const { cssVars, send } = loadGraphScript();
  send({ type: "theme_update", colors: HOST_PALETTE });

  // graph.css styles panels off --color-success/--color-danger too; the handler
  // used to forward only six of the eight, so those two stayed on the Navy-dark
  // fallbacks baked into the stylesheet no matter which preset was active.
  for (const [k, v] of Object.entries(HOST_PALETTE)) {
    assert.equal(cssVars.get(`--color-${k}`), v, `--color-${k} must reach the iframe`);
  }
});

test("a theme_update switches the iframe's color-scheme with the host's", () => {
  const { cssVars, send } = loadGraphScript();

  // Light/dark is not recoverable from the eight colour tokens, so the host
  // sends `mode` alongside them. Without it the guest's native controls — the
  // Date range pickers, the checkboxes, the force sliders, the panel scrollbar
  // — stay on whatever scheme the load started with while the palette around
  // them flips, which is the drift src/styles/tokens.css:13 calls out.
  send({ type: "theme_update", colors: HOST_PALETTE, mode: "light" });
  assert.equal(cssVars.get("color-scheme"), "light");

  send({ type: "theme_update", colors: HOST_PALETTE, mode: "dark" });
  assert.equal(cssVars.get("color-scheme"), "dark");

  // An absent or unrecognised mode leaves the current scheme alone rather than
  // writing an invalid declaration.
  send({ type: "theme_update", colors: HOST_PALETTE });
  assert.equal(cssVars.get("color-scheme"), "dark");
  send({ type: "theme_update", colors: HOST_PALETTE, mode: "sepia" });
  assert.equal(cssVars.get("color-scheme"), "dark");
});

test("node colours follow the host palette instead of a hardcoded copy", () => {
  const { send, probe } = loadGraphScript();

  // Standalone / pre-handshake: getThemeColors()'s own fallbacks.
  assert.equal(probe().nodeColors.paper, "#5b8dee");

  send({ type: "theme_update", colors: HOST_PALETTE });
  const c = probe().nodeColors;
  assert.equal(c.paper, HOST_PALETTE.accent);
  assert.equal(c.tag, HOST_PALETTE.success);
  assert.equal(c.highlight, HOST_PALETTE.danger);
  // Authors have no theme token; the hue is fixed and shape carries the type.
  assert.equal(c.author, "#e8a838");
});

test("cytoscape node styles are rebuilt from the updated palette", () => {
  const { ctx, send } = loadGraphScript();
  send({ type: "theme_update", colors: HOST_PALETTE });

  const byType = new Map<string, any>(
    ctx.cytoscapeStyle()
      .filter((r: any) => r.selector.startsWith("node"))
      .map((r: any) => [/"(\w+)"/.exec(r.selector)![1], r.style])
  );
  assert.equal(byType.get("paper")["background-color"], HOST_PALETTE.accent);
  assert.equal(byType.get("tag")["background-color"], HOST_PALETTE.success);
  assert.equal(byType.get("paper")["text-outline-color"], HOST_PALETTE.bg);
});

test("a theme_update repaints the paper nodes, whose colour is a style bypass", () => {
  const { ctx, send } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});

  // loadGraph -> _applyFilter -> _applyAllStyles paints selection and filter
  // state with per-element `ele.style({...})` calls, which cytoscape stores as
  // bypasses on the element.
  assert.equal(recorder.bypasses.get("1")!["background-color"], "#5b8dee");

  send({ type: "theme_update", colors: HOST_PALETTE });

  // Reinstalling the stylesheet is not enough: Style.clear() runs
  // cleanElements(eles, keepBypasses = true), so the bypass keeps outranking
  // the new sheet and the papers would stay on the previous preset's accent
  // while every other element followed the new one.
  assert.equal(recorder.bypasses.get("1")!["background-color"], HOST_PALETTE.accent);
  assert.equal(recorder.bypasses.get("2")!["background-color"], HOST_PALETTE.accent);
});

test("a theme_update repaints a selected paper with the new highlight colour", () => {
  const { ctx, send } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  ctx._toggleSelection("1");
  assert.equal(recorder.bypasses.get("1")!["background-color"], "#e05c6c");

  send({ type: "theme_update", colors: HOST_PALETTE });
  assert.equal(recorder.bypasses.get("1")!["background-color"], HOST_PALETTE.danger);
  // ...and the repaint must not drop the dim the selection puts on the rest.
  assert.equal(recorder.bypasses.get("2")!["opacity"], 0.28);
});

test("isolate mode takes the hidden elements out of hit-testing, not just out of view", () => {
  const { ctx } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  // "Show highlighted only": paper 1 (cs.LG) matches, paper 2 (cs.CL) does not.
  ctx.filterGraph({ category: "cs.LG", isolate: true });

  // cytoscape hit-tests on `events` / `visibility` / `display` and never on
  // opacity, so an element at opacity 0 stays clickable, hoverable and
  // draggable: tapping blank canvas over paper 2 would navigate to a paper the
  // user cannot see, and would swallow the background tap that clears the
  // selection.
  assert.equal(recorder.bypasses.get("2")!.opacity, 0);
  assert.equal(recorder.bypasses.get("2")!.events, "no");
  assert.equal(recorder.bypasses.get("tag::ml")!.events, "yes", "tag of a matching paper stays live");
  assert.equal(recorder.bypasses.get("1")!.events, "yes");
  assert.equal(recorder.edgeBypasses.get("1->tag::ml")!.events, "yes");
});

test("leaving isolate mode makes the hidden elements clickable again", () => {
  const { ctx } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  ctx.filterGraph({ category: "cs.LG", isolate: true });
  assert.equal(recorder.bypasses.get("2")!.events, "no");

  // `events` is a per-element bypass like the opacity beside it, so a pass that
  // leaves it unset keeps whatever the previous filter wrote — the node would
  // stay dead at 8% opacity, visible but unclickable.
  ctx.filterGraph({ category: "cs.LG", isolate: false });
  assert.equal(recorder.bypasses.get("2")!.opacity, 0.08);
  assert.equal(recorder.bypasses.get("2")!.events, "yes");
});

// ── Visibility checkboxes: a view toggle, not an attribute filter ───────────
// Author and tag visibility is derived purely from edges to matching papers, so
// folding "don't draw papers" into the paper match set took every author and
// tag down with it and left the user staring at an empty canvas.

test("unchecking Papers hides the papers and keeps the authors and tags on screen", () => {
  const { ctx } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});

  ctx.filterGraph({ showPapers: false });

  assert.equal(recorder.bypasses.get("1")!.opacity, 0);
  assert.equal(recorder.bypasses.get("1")!.events, "no");
  assert.equal(recorder.bypasses.get("2")!.opacity, 0);
  // The whole point of the checkbox: what is left is still drawn.
  assert.equal(recorder.bypasses.get("author::7")!.opacity, 1);
  assert.equal(recorder.bypasses.get("author::7")!.events, "yes");
  assert.equal(recorder.bypasses.get("tag::ml")!.opacity, 1);
  assert.equal(recorder.bypasses.get("tag::ml")!.events, "yes");
  // Every edge has a paper endpoint, so none may dangle into empty canvas.
  assert.equal(recorder.edgeBypasses.get("1->author::7")!.opacity, 0);
  assert.equal(recorder.edgeBypasses.get("1->author::7")!.events, "no");
});

test("a switched-off type is removed rather than ghosted at the filter dim", () => {
  const { ctx } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});

  ctx.filterGraph({ showTags: false });

  // 0.08 is what an attribute filter does to a non-matching node. A Visibility
  // checkbox means "don't draw this", so a faint tag cloud is still the noise
  // the user just asked to be rid of.
  assert.equal(recorder.bypasses.get("tag::ml")!.opacity, 0);
  assert.equal(recorder.bypasses.get("tag::ml")!.events, "no");
  assert.equal(recorder.edgeBypasses.get("1->tag::ml")!.opacity, 0);
  // Untouched types keep full opacity, including the author edge.
  assert.equal(recorder.bypasses.get("1")!.opacity, 1);
  assert.equal(recorder.bypasses.get("author::7")!.opacity, 1);
  assert.equal(recorder.edgeBypasses.get("1->author::7")!.opacity, 1);
});

test("hiding a type leaves the layout alone; an attribute filter still pins", () => {
  const { ctx, probe } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});

  // Nodes an attribute filter excluded are pinned so they stop pushing the
  // matching ones around — paper 2 is cs.CL, so it freezes.
  // `fx` is undefined until a pin sets it and null once one is released, so
  // normalise before asserting "not pinned".
  const pin = (id: string) => probe().simNodeById.get(id).fx ?? null;
  ctx.filterGraph({ category: "cs.LG" });
  assert.notEqual(pin("2"), null);

  // A type toggle is a view change: pinning what it hides would drop every link
  // through it (all edges run through a paper) and let the rest fly apart under
  // pure repulsion, so the graph the user unhides has been rearranged.
  ctx.filterGraph({ showPapers: false });
  assert.equal(pin("1"), null);
  assert.equal(pin("2"), null);
  assert.equal(pin("author::7"), null);
});

test("Select all skips papers the user has switched off", () => {
  const { ctx, posted } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  ctx.filterGraph({ showPapers: false });

  ctx.selectAllPapers();

  // Without the guard this selects both papers and hands the host two source
  // ids for rows the user cannot see on the canvas.
  const sel = posted.filter((m: any) => m.type === "selection_changed").pop();
  assert.deepEqual([...sel.sourceIds], []);
});

// ── A selection under an active filter ──────────────────────────────────────
// An attribute filter leaves what it excludes at 8% opacity, and _eventsFor
// only drops a node out of hit-testing at opacity 0 — so every ghost on the
// canvas is still fully clickable and can enter the selection, and a selection
// built BEFORE the filter was typed survives the pass unchanged. The host acts
// on that set ("N selected" / "Add to Project"), so it has to be readable on
// the canvas: the highlight belongs to the selection, the dim to the filter.

test("Ctrl-clicking a ghost the filter excluded shows up on the canvas", () => {
  const { ctx, probe } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  // Paper 1 is cs.LG and matches; paper 2 is cs.CL and is left as an 8% ghost.
  ctx.filterGraph({ category: "cs.LG" });
  assert.equal(recorder.bypasses.get("2")!.events, "yes", "the ghost is still clickable");

  emitCy("tap", recorder.cyNodeById.get("2"), 'node[type = "paper"]', { ctrlKey: true });

  assert.ok(probe().selectedIds.has("2"));
  // Without the paint this click changed NOTHING on screen: the header count
  // and the host's action bar both went up by one with no way to tell which
  // paper had joined.
  assert.equal(recorder.bypasses.get("2")!["background-color"], probe().nodeColors.highlight);
  // The filter still owns opacity, so the ghost stays a ghost.
  assert.equal(recorder.bypasses.get("2")!.opacity, 0.08);
});

test("a selection the filter then excludes stays readable on the canvas", () => {
  const { ctx, probe } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  ctx._toggleSelection("1");
  assert.equal(recorder.bypasses.get("1")!["background-color"], probe().nodeColors.highlight);

  // Typing a filter that excludes it must not quietly un-paint a paper the
  // host is still counting and about to add to a project.
  ctx.filterGraph({ category: "cs.CL" });

  assert.ok(probe().selectedIds.has("1"));
  assert.equal(recorder.bypasses.get("1")!["background-color"], probe().nodeColors.highlight);
  assert.equal(recorder.bypasses.get("1")!.opacity, 0.08);
});

test("the count names the selected papers isolate mode takes off the canvas", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  const count = () => document.getElementById("selectionCount").textContent;
  ctx.selectAllPapers();
  assert.equal(count(), "(2)");

  // "Show highlighted only" takes the non-matching paper to opacity 0, where
  // the highlight cannot report it — so the count has to.
  ctx.filterGraph({ category: "cs.LG", isolate: true });
  assert.equal(recorder.bypasses.get("2")!.opacity, 0);
  assert.equal(count(), "(2, 1 hidden)");

  // ...and stops saying so once the paper is drawn again.
  ctx.filterGraph({ category: "cs.LG", isolate: false });
  assert.equal(count(), "(2)");
});

test("switching Papers off reports the whole selection as hidden", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  const count = () => document.getElementById("selectionCount").textContent;
  ctx.selectAllPapers();

  ctx.filterGraph({ showPapers: false });

  // Select-all refuses to ADD papers the user cannot see (test above); a
  // selection made before the checkbox was unticked is still there, and the
  // host's "2 selected" bar still acts on it.
  assert.equal(count(), "(2, 2 hidden)");
});

test("an ordinary attribute filter does not call its ghosts hidden", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  const count = () => document.getElementById("selectionCount").textContent;
  ctx.selectAllPapers();

  // Paper 2 is excluded but still drawn, and now still painted as selected, so
  // there is nothing the count needs to explain. "hidden" means opacity 0.
  ctx.filterGraph({ category: "cs.LG" });
  assert.equal(count(), "(2)");
});

// ── Host-driven selection (`set_selection`) ─────────────────────────────────
// The project picker's partial-failure contract (src/lib/paperMutations.ts)
// re-selects exactly the papers that could not be added, so a retry can't
// re-add the ones that made it in. GraphPage cannot honour that on its own: the
// selection lives in the guest and the page only mirrors it, so the narrowing
// has to cross the frame.

/** The last `selection_changed` payload, as a plain host-realm array. */
const lastSelection = (posted: any[]) => {
  const m = posted.filter((p) => p.type === "selection_changed").pop();
  return m ? [...m.sourceIds].map(String) : null;
};

test("the host can narrow the guest's selection to the papers that failed", () => {
  const { ctx, posted, probe, send } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  ctx.selectAllPapers();
  assert.deepEqual(lastSelection(posted), ["arxiv:2204.1", "arxiv:2204.2"]);

  // "1 of 2 papers could not be added" — the picker stays open re-selecting
  // just the failure.
  send({ type: "set_selection", sourceIds: ["arxiv:2204.2"] });

  assert.deepEqual([...probe().selectedIds], ["2"]);
  assert.deepEqual(lastSelection(posted), ["arxiv:2204.2"]);
  // The canvas has to agree: the paper that DID make it in is no longer drawn
  // as selected.
  const { highlight, paper } = probe().nodeColors;
  assert.equal(recorder.bypasses.get("2")!["background-color"], highlight);
  assert.equal(recorder.bypasses.get("1")!["background-color"], paper);
});

test("a narrowed selection is what the next click in the graph builds on", () => {
  const { ctx, posted, probe, send } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  ctx.selectAllPapers();
  send({ type: "set_selection", sourceIds: ["arxiv:2204.2"] });

  // This is the failure the message exists to stop: without it the guest still
  // held both papers, so the next Ctrl-click posted the WHOLE original set back
  // over the host's narrowed copy and the retry re-added the paper that had
  // already been added.
  emitCy("tap", recorder.cyNodeById.get("1"), 'node[type = "paper"]', { ctrlKey: true });

  assert.deepEqual([...probe().selectedIds], ["2", "1"]);
  assert.deepEqual(lastSelection(posted), ["arxiv:2204.2", "arxiv:2204.1"]);
});

test("a source id with no node on the canvas is reported back as unselected", () => {
  const { ctx, posted, probe, send } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  ctx.selectAllPapers();

  // A paper deleted (or dropped from the payload) since the load cannot be
  // selected in here, so the host's count must not keep claiming it is —
  // the reply is what actually happened, not an echo of the ask.
  send({ type: "set_selection", sourceIds: ["arxiv:2204.2", "arxiv:gone"] });

  assert.deepEqual([...probe().selectedIds], ["2"]);
  assert.deepEqual(lastSelection(posted), ["arxiv:2204.2"]);
});

test("a set_selection without a source id list is ignored", () => {
  const { ctx, posted, probe, send } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  ctx.selectAllPapers();
  const before = posted.filter((m: any) => m.type === "selection_changed").length;

  send({ type: "set_selection" });

  // Nothing to apply is not "clear it": a malformed message must not silently
  // drop a selection the user built.
  assert.deepEqual([...probe().selectedIds], ["1", "2"]);
  assert.equal(posted.filter((m: any) => m.type === "selection_changed").length, before);
});

// ── Node clicks: what each of the three node types does when tapped ─────────

const TAG_NODE = { id: () => "tag::ml", data: (k: string) => ({ id: "tag::ml", label: "ML", type: "tag" } as any)[k] };
const AUTHOR_NODE = { id: () => "author::7", data: (k: string) => ({ id: "author::7", label: "Ada Lovelace", type: "author", author_id: 7 } as any)[k] };

test("tapping a tag node opens its tag page", () => {
  const { ctx, posted } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});

  emitCy("tap", TAG_NODE, 'node[type = "tag"]');

  // Papers and authors have always navigated; a tag was inert, with no cue on
  // the canvas that it was. The host routes this to /tags/:label, the same page
  // TagBadge links to everywhere else.
  const msg = posted.filter((m: any) => m.type === "tag_clicked").pop();
  assert.ok(msg, "tapping a tag must ask the host to navigate");
  assert.equal(msg.label, "ML");
});

test("Ctrl-clicking a tag node does not navigate", () => {
  const { ctx, posted } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});

  // Ctrl/Cmd is reserved for additive paper selection, so it must not fire a
  // navigation out from under a multi-select the user is halfway through.
  emitCy("tap", TAG_NODE, 'node[type = "tag"]', { ctrlKey: true });
  emitCy("tap", TAG_NODE, 'node[type = "tag"]', { metaKey: true });

  assert.equal(posted.filter((m: any) => m.type === "tag_clicked").length, 0);
});

test("leaving the graph via an author or tag node drops the selection behind it", () => {
  const { ctx, document, probe } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  const count = () => document.getElementById("selectionCount").textContent;

  for (const [node, sel] of [[AUTHOR_NODE, 'node[type = "author"]'], [TAG_NODE, 'node[type = "tag"]']] as const) {
    ctx._toggleSelection("1");
    ctx._toggleSelection("2");
    assert.equal(probe().selectedIds.size, 2);

    emitCy("tap", node, sel);

    // The host clears its own copy when it navigates and AppShell keeps this
    // iframe alive, so a selection left here comes back highlighted with a
    // counter the host disagrees with and no action bar to act on it. Only the
    // paper click used to clear it.
    assert.equal(probe().selectedIds.size, 0);
    assert.equal(count(), "(0)");
  }
});

// ── graph_loaded: what the host needs to tell blank-canvas states apart ──────
// The iframe is a bare cytoscape canvas with no loading / empty / error surface
// of its own, so GraphPage overlays the app's Spinner and EmptyState on top of
// it. It can only pick between them from this reply: `ok` alone cannot separate
// "the library is empty" from "the graph rendered fine".

test("a successful load reports how many nodes it drew", async () => {
  const { ctx, posted } = loadGraphScript();
  await ctx.fetchAndLoadGraph({ preserveView: false });

  const reply = posted.filter((m: any) => m.type === "graph_loaded").pop();
  assert.equal(reply.ok, true);
  assert.equal(reply.nodeCount, PAYLOAD.nodes.length);
});

test("an empty library is reported as a load that drew nothing", async () => {
  const { ctx, posted } = loadGraphScript();
  ctx.fetch = async (url: string) => {
    const path = url.split("?")[0].replace("http://localhost:5180", "");
    const body = path === "/api/graph" ? { nodes: [], edges: [] } : API_RESPONSES[path];
    return { ok: true, status: 200, json: async () => structuredClone(body) };
  };
  await ctx.fetchAndLoadGraph({ preserveView: false });

  // ok:true with nodeCount 0 — a fresh install, not a failure. Without the
  // count the host cannot say so and the user gets an unexplained blank canvas.
  const reply = posted.filter((m: any) => m.type === "graph_loaded").pop();
  assert.equal(reply.ok, true);
  assert.equal(reply.nodeCount, 0);
});

test("a failed load reports why, not just that it failed", async () => {
  const { ctx, posted } = loadGraphScript();
  ctx.fetch = async () => ({ ok: false, status: 503 });
  await ctx.fetchAndLoadGraph({ preserveView: false });

  const reply = posted.filter((m: any) => m.type === "graph_loaded").pop();
  assert.equal(reply.ok, false);
  assert.match(reply.error, /HTTP 503/);
});

// ── Only /api/graph is required ──────────────────────────────────────────────
// The other three endpoints fill the filter datalists and nothing else, so a
// 500 from one of them used to fail the whole load — `Promise.all` over four
// requests, one validation, one error reply — and the host put its "Couldn't
// load the graph" EmptyState over a canvas whose data had arrived intact.

/** A fetch stub serving `routes`, with any path in `down` answering 500. */
function fetchServing(routes: Record<string, unknown>, down: string[] = []) {
  return async (url: string) => {
    const at = url.indexOf("/api");
    const path = at === -1 ? url : url.slice(at).split("?")[0];
    if (down.includes(path)) return { ok: false, status: 500 };
    const body = routes[path];
    if (!body) return { ok: false, status: 404 };
    return { ok: true, status: 200, json: async () => structuredClone(body) };
  };
}

test("a dropdown endpoint that is down does not fail the whole graph load", async () => {
  const { ctx, posted } = loadGraphScript();
  ctx.fetch = fetchServing(API_RESPONSES, ["/api/tags"]);
  await ctx.fetchAndLoadGraph({ preserveView: false });

  // The nodes and edges arrived; only the Paper Tags datalist did not.
  const reply = posted.filter((m: any) => m.type === "graph_loaded").pop();
  assert.equal(reply.ok, true);
  assert.equal(reply.nodeCount, PAYLOAD.nodes.length);
});

test("a dropdown endpoint that is down keeps the options the last load installed", async () => {
  const { ctx, document } = loadGraphScript();
  await ctx.fetchAndLoadGraph({ preserveView: false });
  const tagList = document.getElementById("tagList").innerHTML;

  ctx.fetch = fetchServing(
    { ...API_RESPONSES, "/api/categories": { categories: ["cs.AI"] } },
    ["/api/tags", "/api/graph/project-options"]
  );
  await ctx.fetchAndLoadGraph({ preserveView: true });

  // Blanking the list would read as "this library has no tags any more", which
  // is a different claim from "that request failed".
  assert.equal(document.getElementById("tagList").innerHTML, tagList);
  // ...while the list that DID answer refreshes, which is the half of this
  // reload that used to be thrown away along with the failed request.
  assert.equal(document.getElementById("categoryList").innerHTML, '<option value="cs.AI">');
});

test("the graph payload itself is still required", async () => {
  const { ctx, posted } = loadGraphScript();
  ctx.fetch = fetchServing(API_RESPONSES, ["/api/graph"]);
  await ctx.fetchAndLoadGraph({ preserveView: false });

  const reply = posted.filter((m: any) => m.type === "graph_loaded").pop();
  assert.equal(reply.ok, false);
  assert.match(reply.error, /HTTP 500/);
});

// ── hasGraph: whether a failure took the canvas with it ──────────────────────
// Everything before loadGraph() -- the four requests and the payload validation
// -- runs while the settled canvas is untouched, so the common failure (a
// backend answering 500 to a Refresh) leaves a perfectly good graph on screen.
// The host cannot tell that from a failure that DID destroy it, and covering a
// live graph with "Couldn't load the graph" throws away the layout and the
// framing the user is looking at. See src/lib/graphLoadState.ts for the rule
// this field feeds.

test("a reload that fails before it redraws reports the canvas as still there", async () => {
  const { ctx, posted } = loadGraphScript();
  await ctx.fetchAndLoadGraph({ preserveView: false });
  assert.equal(recorder.cyCreated, 1);

  ctx.fetch = fetchServing(API_RESPONSES, ["/api/graph"]);
  await ctx.fetchAndLoadGraph({ preserveView: true });

  // The graph request threw out of the Promise.all, so loadGraph never ran and
  // the instance built by the first load is the one still on the canvas.
  assert.equal(recorder.cyCreated, 1);
  const reply = posted.filter((m: any) => m.type === "graph_loaded").pop();
  assert.equal(reply.ok, false);
  assert.equal(reply.hasGraph, true);
});

test("a first load that fails reports that there is no canvas behind it", async () => {
  const { ctx, posted } = loadGraphScript();
  ctx.fetch = fetchServing(API_RESPONSES, ["/api/graph"]);
  await ctx.fetchAndLoadGraph({ preserveView: false });

  const reply = posted.filter((m: any) => m.type === "graph_loaded").pop();
  assert.equal(reply.ok, false);
  assert.equal(reply.hasGraph, false);
});

test("a reload that dies inside loadGraph reports the canvas as gone", async () => {
  const { ctx, posted } = loadGraphScript();
  await ctx.fetchAndLoadGraph({ preserveView: false });

  // loadGraph destroys the outgoing instance BEFORE it builds the new one, so a
  // throw from in there really does leave a blank canvas -- the one failure the
  // host must still escalate even though a graph was on screen a moment ago.
  ctx.cytoscape = () => { throw new Error("renderer died"); };
  await ctx.fetchAndLoadGraph({ preserveView: true });

  const reply = posted.filter((m: any) => m.type === "graph_loaded").pop();
  assert.equal(reply.ok, false);
  assert.equal(reply.hasGraph, false);
});

test("the reloaded project options are in place before the filter pass that reads them", async () => {
  const { ctx, probe } = loadGraphScript();
  const graph = {
    nodes: [{ id: 1, source_id: "arxiv:1", label: "In project", type: "paper", project_ids: [5] }],
    edges: [],
  };
  const withProjectTags = (tags: string[]) => ({
    ...API_RESPONSES,
    "/api/graph": graph,
    "/api/graph/project-options": { projects: [{ id: 5, name: "Thesis", color: "#5b8dee", tags }] },
  });

  ctx.fetch = fetchServing(withProjectTags([]));
  await ctx.fetchAndLoadGraph({ preserveView: false });

  // The user filters by a project tag, then someone adds that tag to the
  // project elsewhere in the app and they hit Refresh.
  probe().projTagFilterNames.push("urgent");
  ctx._applyFilter();
  assert.deepEqual([...probe().visiblePaperIds], []);

  ctx.fetch = fetchServing(withProjectTags(["urgent"]));
  await ctx.fetchAndLoadGraph({ preserveView: true });

  // loadGraph ends in a filter pass, and the Project Tags rows resolve through
  // _projectMap — so installing the new options after it left the fresh graph
  // filtered by the PREVIOUS load's project tags until a control was touched.
  assert.deepEqual([...probe().visiblePaperIds], ["1"]);
});

test("a guest with no backend to fetch from says so at once", () => {
  // file: is the only base that resolves to null. The bootstrap used to return
  // silently there, so the host got no `graph_loaded` at all and sat on the
  // spinner until its 8s dropped-reply fallback escalated it.
  const { posted, fetched } = loadGraphScript({
    location: { protocol: "file:", hostname: "", origin: "null" },
  });

  assert.equal(fetched.length, 0);
  const reply = posted.filter((m: any) => m.type === "graph_loaded").pop();
  assert.equal(reply.ok, false);
  assert.equal(reply.error, "No backend to fetch from");
  // Nothing was ever drawn, so this failure has no canvas to protect.
  assert.equal(reply.hasGraph, false);
});

// ── Webfont gate ─────────────────────────────────────────────────────────────

/** Drain the microtask queue (and any already-due timers). */
const tick = () => new Promise((r) => setImmediate(r));

test("the first render waits for the app's webfont before measuring labels", async () => {
  let release!: () => void;
  const requested: string[] = [];
  const { posted } = loadGraphScript({
    fonts: {
      load(spec: string) {
        requested.push(spec);
        return new Promise((res) => {
          release = () => res(undefined);
        });
      },
    },
  });

  // Cytoscape caches each label's measured size on the renderer, keyed by the
  // text plus the font style properties — not by whether the family had
  // arrived. Rendering before Inter loads bakes the fallback face's metrics in
  // for the session (tag chips sized for the wrong glyphs, text-max-width
  // ellipsizing at the wrong point), and no later restyle clears it.
  await tick();
  await tick();
  assert.equal(recorder.cyCreated, 0, "must not render while the label font is pending");
  assert.equal(requested.length, 1, "must request the label font exactly once per load");
  assert.match(requested[0], /\bInter\b/);

  release();
  await tick();
  await tick();
  assert.equal(recorder.cyCreated, 1);
  // Objects out of the vm realm never deep-equal a host literal, so compare
  // the fields the host actually branches on.
  const loaded = posted.filter((m: any) => m.type === "graph_loaded");
  assert.equal(loaded.length, 1);
  assert.equal(loaded[0].ok, true);
  assert.equal(loaded[0].nodeCount, PAYLOAD.nodes.length);
});

test("a webfont that never resolves still renders, in the fallback face", async () => {
  const { posted } = loadGraphScript({ fonts: { load: () => new Promise(() => {}) } });
  await tick();
  assert.equal(recorder.cyCreated, 0);

  // graph.js races the font against FONT_LOAD_TIMEOUT_MS; run the timer out
  // rather than leaving the canvas blank forever behind a stalled request.
  const timeout = Number(
    fs.readFileSync(GRAPH_JS, "utf8").match(/FONT_LOAD_TIMEOUT_MS = (\d+)/)![1]
  );
  await new Promise((r) => setTimeout(r, timeout + 20));
  assert.equal(recorder.cyCreated, 1, "a stalled font request must not block the graph");
  // Objects out of the vm realm never deep-equal a host literal, so compare
  // the fields the host actually branches on.
  const loaded = posted.filter((m: any) => m.type === "graph_loaded");
  assert.equal(loaded.length, 1);
  assert.equal(loaded[0].ok, true);
  assert.equal(loaded[0].nodeCount, PAYLOAD.nodes.length);
});

// ── Which backend the guest fetches from ────────────────────────────────────
// `tauri dev` and browser dev both serve this document from
// http://localhost:5180, but the app around it reads two different libraries
// there: invoke() into the in-process one under Tauri, the Vite /api proxy to a
// separate dev server on :8000 in a browser. The guest cannot tell them apart,
// so the host names the transport in the src (src/lib/graphIframeSrc.ts) and
// graph.js's bootstrap reads it. It used to sniff its own URL instead, which
// made the graph the one surface in the app pointed at the dev server's
// database under `tauri dev` — a different library, usually not running at all.

/** The `/api/graph` request the bootstrap fired, if any. */
function graphRequest(fetched: string[]): string | undefined {
  return fetched.find((u) => u.includes("/api/graph") && !u.includes("project-options"));
}

test("api=linxiv fetches the in-process backend, not the document's own origin", () => {
  // The `tauri dev` case: served from localhost:5180 like browser dev, but the
  // library lives in-process behind the custom scheme.
  const { fetched } = loadGraphScript({ search: "?api=linxiv" });
  assert.equal(graphRequest(fetched), "linxiv://localhost/api/graph");
  // Every endpoint, not just the graph one — the datalists come from the same
  // library and would otherwise offer another database's tags.
  assert.ok(fetched.length >= 4 && fetched.every((u) => u.startsWith("linxiv://localhost/")));
});

test("api=linxiv follows the platform's custom-scheme host form", () => {
  // Windows serves it as http://linxiv.localhost (Tauri docs), the same split
  // src/api/papers.ts makes in linxivUrl().
  const { fetched } = loadGraphScript({
    search: "?api=linxiv",
    userAgent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64)",
  });
  assert.equal(graphRequest(fetched), "http://linxiv.localhost/api/graph");
});

test("api=origin fetches through the dev server the document came from", () => {
  const { fetched } = loadGraphScript({ search: "?api=origin" });
  assert.equal(graphRequest(fetched), "http://localhost:5180/api/graph");
});

test("the transport rides alongside the options the src already carried", () => {
  // Both are read off the same query in one pass; adding ?api= must not cost
  // the bootstrap its ?excludeSingleAuthors=.
  const { fetched } = loadGraphScript({ search: "?api=linxiv&excludeSingleAuthors=1&mode=dark" });
  assert.equal(
    graphRequest(fetched),
    "linxiv://localhost/api/graph?exclude_single_authors=true"
  );
});

test("with no transport named the guest falls back to sniffing its own URL", () => {
  // Opened standalone there is no host to ask. A dev server serves its own
  // /api; anything else is the packaged app, whose only reachable backend is
  // the custom scheme.
  assert.equal(
    graphRequest(loadGraphScript().fetched),
    "http://localhost:5180/api/graph"
  );
  assert.equal(
    graphRequest(
      loadGraphScript({
        location: { protocol: "tauri:", hostname: "localhost", origin: "tauri://localhost" },
      }).fetched
    ),
    "linxiv://localhost/api/graph"
  );
});

test("an unrecognised transport falls back rather than fetching a bad base", () => {
  const { fetched } = loadGraphScript({ search: "?api=sqlite" });
  assert.equal(graphRequest(fetched), "http://localhost:5180/api/graph");
});

// ── Hover inspector ─────────────────────────────────────────────────────────
// Paper labels are ellipsized at 180px on the canvas, so a real title cannot be
// read there at all, and the category / date / tags / PDF flag / abstract that
// `/api/graph` sends for every paper had no surface: loadGraph copied all of it
// into cytoscape node data and nothing read a byte of it.

/** Emit a cytoscape node event carrying the real stub node graph.js holds. */
function hover(id: string) {
  emitCy("mouseover", recorder.cyNodeById.get(id), "node");
}

test("hovering a paper shows its full title and the metadata the canvas can't", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});

  hover("1");
  const tip = document.getElementById("node-tooltip");
  assert.equal(tip.style.display, "", "the box must be shown");
  assert.equal(document.getElementById("node-tooltip-title").textContent, "Attention");
  const meta = document.getElementById("node-tooltip-meta").textContent as string;
  // The three payload fields the canvas has nowhere to put, on one line...
  assert.ok(meta.includes("cs.LG"), meta);
  assert.ok(meta.includes("2024-01-01"), meta);
  assert.ok(meta.includes("PDF") && !meta.includes("No PDF"), meta);
  // ...then the tags and the abstract, which were fetched and discarded.
  assert.ok(meta.includes("ML") && meta.includes("nlp"), meta);
  assert.ok(meta.includes("We propose a new architecture."), meta);
});

test("the inspector lists the tag chips the canvas drew, not the raw column", () => {
  const { ctx, document } = loadGraphScript();
  // graph.rs trims each tag, drops what is left empty and emits ONE node per
  // `tag::<lower>`, so this paper contributes exactly one chip: "ML".
  ctx.loadGraph(
    {
      nodes: [{ id: 1, source_id: "arxiv:1", label: "T", type: "paper", tags: ["ML", " ml ", "  "] }],
      edges: [],
    },
    {}
  );

  hover("1");
  const meta = document.getElementById("node-tooltip-meta").textContent as string;
  const tagLine = meta.split("\n").find((l: string) => l.includes("ML")) as string;
  assert.equal(tagLine, "ML");
});

test("leaving a node hides the inspector again", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});

  hover("1");
  emitCy("mouseout", recorder.cyNodeById.get("1"), "node");
  assert.equal(document.getElementById("node-tooltip").style.display, "none");
});

test("a pan or zoom drops the inspector rather than stranding it", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});

  hover("1");
  // It is positioned in rendered (screen) coordinates, so the node moves out
  // from under it on any viewport change.
  emitCy("viewport", null);
  assert.equal(document.getElementById("node-tooltip").style.display, "none");

  // Same for a drag, which starts with a grab.
  hover("1");
  emitCy("grab", recorder.cyNodeById.get("1"), "node");
  assert.equal(document.getElementById("node-tooltip").style.display, "none");
});

test("a click that leaves the graph does not leave the inspector behind", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});

  hover("1");
  // AppShell keeps the iframe alive across the route change, so whatever is on
  // screen here is what the user comes back to.
  emitCy("tap", recorder.cyNodeById.get("1"), 'node[type = "paper"]');
  assert.equal(document.getElementById("node-tooltip").style.display, "none");
});

test("author and tag nodes report how many papers they join", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});

  hover("author::7");
  assert.equal(document.getElementById("node-tooltip-title").textContent, "Ada Lovelace");
  // /api/graph sends no degree, so the count comes off the edge list.
  assert.equal(document.getElementById("node-tooltip-meta").textContent, "Author \u00b7 1 paper");

  hover("tag::ml");
  assert.equal(document.getElementById("node-tooltip-title").textContent, "ML");
  assert.equal(document.getElementById("node-tooltip-meta").textContent, "Tag \u00b7 1 paper");
});

// One author and one tag, each joined to BOTH papers, so a filter that keeps
// one paper splits their degree from what the canvas is drawing.
const SHARED_NODE_PAYLOAD = {
  nodes: [
    { id: 1, source_id: "arxiv:1", label: "Attention", type: "paper", category: "cs.LG", tags: ["nlp"], has_pdf: true, published: "2024-01-01", summary: null },
    { id: 2, source_id: "arxiv:2", label: "Other", type: "paper", category: "cs.CL", tags: ["nlp"], has_pdf: false, published: "2024-02-01", summary: null },
    { id: "author::7", label: "Ada Lovelace", type: "author", author_id: 7 },
    { id: "tag::nlp", label: "nlp", type: "tag" },
  ],
  edges: [
    { source: 1, target: "author::7" },
    { source: 2, target: "author::7" },
    { source: 1, target: "tag::nlp" },
    { source: 2, target: "tag::nlp" },
  ],
};

test("a filtered canvas says how much of a shared node's degree it still draws", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(SHARED_NODE_PAYLOAD, {});

  // Unfiltered, the two numbers are the same and the line says it once.
  hover("author::7");
  assert.equal(document.getElementById("node-tooltip-meta").textContent, "Author \u00b7 2 papers");

  // One paper left on the canvas, the other an 8% ghost: reading "2 papers"
  // off a node with a single line leaving it is the same disagreement the
  // Selection counter and the panel badges already close.
  ctx.filterGraph({ category: "cs.LG" });
  hover("author::7");
  assert.equal(
    document.getElementById("node-tooltip-meta").textContent,
    "Author \u00b7 2 papers (1 shown)"
  );
  hover("tag::nlp");
  assert.equal(
    document.getElementById("node-tooltip-meta").textContent,
    "Tag \u00b7 2 papers (1 shown)"
  );
});

test("a shared node whose papers are all filtered out says none are shown", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(SHARED_NODE_PAYLOAD, {});

  // Without isolate the excluded nodes stay 8% ghosts, and _eventsFor only
  // drops a node out of hit-testing at opacity 0 -- so this ghost author is
  // fully hoverable, and "2 papers" was the only thing it had to say.
  ctx.filterGraph({ category: "zzz-no-such-category" });
  hover("author::7");
  assert.equal(
    document.getElementById("node-tooltip-meta").textContent,
    "Author \u00b7 2 papers (none shown)"
  );
});

test("switching Papers off leaves the degree but reports nothing drawn", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(SHARED_NODE_PAYLOAD, {});

  // A Visibility checkbox is not an attribute filter -- every paper still
  // MATCHES -- but none of them is on the canvas, which is what the hovered
  // author is standing in front of.
  ctx.filterGraph({ showPapers: false });
  hover("author::7");
  assert.equal(
    document.getElementById("node-tooltip-meta").textContent,
    "Author \u00b7 2 papers (none shown)"
  );

  // ...and switching them back on restores the plain line.
  ctx.filterGraph({ showPapers: true });
  hover("author::7");
  assert.equal(document.getElementById("node-tooltip-meta").textContent, "Author \u00b7 2 papers");
});

test("an undated paper says so instead of showing the 0001-01-01 sentinel", () => {
  const { ctx, document } = loadGraphScript();
  // PAPER_META.PUBLISHED holds chrono's date.min for a paper with no date and
  // /api/graph forwards the column raw; loadGraph folds it to null.
  ctx.loadGraph(
    {
      nodes: [{ ...PAYLOAD.nodes[0], published: "0001-01-01", has_pdf: false, summary: null }],
      edges: [],
    },
    {}
  );

  hover("1");
  const meta = document.getElementById("node-tooltip-meta").textContent as string;
  assert.ok(meta.includes("No publication date"), meta);
  assert.ok(!meta.includes("0001-01-01"), meta);
  assert.ok(meta.includes("No PDF"), meta);
});

test("a long abstract is truncated rather than filling the viewport", () => {
  const { ctx, document } = loadGraphScript();
  const summary = "word ".repeat(400);
  ctx.loadGraph({ nodes: [{ ...PAYLOAD.nodes[0], summary }], edges: [] }, {});

  hover("1");
  const meta = document.getElementById("node-tooltip-meta").textContent as string;
  assert.ok(meta.length < 400, `abstract must be cut down, got ${meta.length} chars`);
  assert.ok(meta.endsWith("\u2026"), meta.slice(-20));
});

test("reloading the graph clears an inspector pinned to a node that is gone", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  hover("1");

  // Refresh / option toggle destroys and rebuilds every element.
  ctx.loadGraph(PAYLOAD, { preserveView: true });
  assert.equal(document.getElementById("node-tooltip").style.display, "none");
});

// ── App keyboard shortcuts ──────────────────────────────────────────────────
// Key events do not cross a frame boundary, so useGlobalShortcuts (bound on the
// HOST window by AppShell) sees nothing while the graph has focus — which it
// takes on the first canvas click, since panning and selecting both need one.
// Every shortcut Settings lists as app-wide was therefore dead on /graph, and
// the webview zoom the host listener suppresses with preventDefault took over
// instead. The host pushes its combos and the guest hands the matches back.

/** A keydown event object shaped like the fields graph.js reads, with a
 *  recorder for the default it must swallow. */
function keydown(key: string, mods: { ctrl?: boolean; meta?: boolean; alt?: boolean; shift?: boolean } = {}) {
  const e = {
    key,
    ctrlKey: !!mods.ctrl,
    metaKey: !!mods.meta,
    altKey: !!mods.alt,
    shiftKey: !!mods.shift,
    defaultPrevented: false,
    preventDefault() { e.defaultPrevented = true; },
  };
  return e;
}

const shortcutPosts = (posted: any[]) => posted.filter((m) => m.type === "shortcut_key");

// The posted object is built inside the vm realm, so deepEqual against a host
// literal fails on realm identity alone (Object from a different context).
// Compare the fields the host branches on, as the other postMessage tests do.
function assertCombo(
  actual: any,
  expected: { ctrl: boolean; alt: boolean; shift: boolean; key: string }
) {
  assert.equal(actual.ctrl, expected.ctrl);
  assert.equal(actual.alt, expected.alt);
  assert.equal(actual.shift, expected.shift);
  assert.equal(actual.key, expected.key);
}

test("a shortcut keydown inside the graph is swallowed and handed back to the host", () => {
  const { posted, send, fire } = loadGraphScript();
  send({ type: "set_shortcuts", combos: activeShortcutCombos({}) });

  const e = keydown("-", { ctrl: true });
  fire("keydown", e);

  // Swallowed so the webview's own zoom can't fire alongside the interface
  // zoom the host is about to apply — src/lib/zoom.ts's two mechanisms must
  // never compound.
  assert.equal(e.defaultPrevented, true);
  const [msg] = shortcutPosts(posted);
  assert.ok(msg, "the guest must hand the keydown back to the host");
  assertCombo(msg.combo, { ctrl: true, alt: false, shift: false, key: "-" });
});

test("the guest forwards every combo the host's shortcut registry answers to", () => {
  // The real cross-file guard: whatever src/lib/shortcuts.ts currently binds
  // has to survive the trip through graph.js's own matcher, including the
  // Shift-either-way spellings ('+' is Shift+'=' on most layouts).
  for (const combo of activeShortcutCombos({})) {
    for (const shift of combo.shift === null ? [false, true] : [combo.shift]) {
      const { posted, send, fire } = loadGraphScript();
      send({ type: "set_shortcuts", combos: activeShortcutCombos({}) });
      const e = keydown(combo.key, { ctrl: combo.ctrl, alt: combo.alt, shift });
      fire("keydown", e);
      assert.equal(e.defaultPrevented, true, `Ctrl+${combo.key} (shift=${shift}) was not intercepted`);
      assert.equal(shortcutPosts(posted).length, 1, `Ctrl+${combo.key} was not forwarded`);
    }
  }
});

test("Cmd counts as Ctrl, and the key matches whatever case the layout reports", () => {
  const { posted, send, fire } = loadGraphScript();
  send({ type: "set_shortcuts", combos: [{ ctrl: true, alt: false, shift: false, key: "k" }] });

  // captureOverride() folds Ctrl and Cmd into one modifier; graph.js has to
  // make the same fold or the shortcut dies on macOS only.
  const e = keydown("K", { meta: true });
  fire("keydown", e);
  assert.equal(e.defaultPrevented, true);
  assertCombo(shortcutPosts(posted)[0].combo, { ctrl: true, alt: false, shift: false, key: "K" });
});

test("keys outside the pushed list are left entirely alone", () => {
  const { posted, send, fire } = loadGraphScript();
  send({ type: "set_shortcuts", combos: activeShortcutCombos({}) });

  // Ctrl+A / Ctrl+C are how the filter and tag boxes are edited; swallowing
  // them to look for a shortcut would break typing in the panel.
  for (const e of [keydown("a", { ctrl: true }), keydown("c", { ctrl: true }),
                   keydown("0"), keydown("0", { ctrl: true, alt: true })]) {
    fire("keydown", e);
    assert.equal(e.defaultPrevented, false, `${e.key} must keep its default`);
  }
  assert.deepEqual(shortcutPosts(posted), []);
});

test("nothing is intercepted before the host has pushed its combos", () => {
  // A standalone load has no host to ask, and the list starts empty — the
  // guest must not guess at a vocabulary and swallow keys for nobody.
  const { posted, fire } = loadGraphScript();
  const e = keydown("-", { ctrl: true });
  fire("keydown", e);
  assert.equal(e.defaultPrevented, false);
  assert.deepEqual(shortcutPosts(posted), []);
});

test("a rebind replaces the combo the guest intercepts", () => {
  const { posted, send, fire } = loadGraphScript();
  const overrides = { "zoom-in": { ctrl: true, alt: true, shift: false, key: "k" } };
  send({ type: "set_shortcuts", combos: activeShortcutCombos(overrides) });

  const rebound = keydown("k", { ctrl: true, alt: true });
  fire("keydown", rebound);
  assert.equal(rebound.defaultPrevented, true);
  assertCombo(shortcutPosts(posted)[0].combo, { ctrl: true, alt: true, shift: false, key: "k" });

  // The chord it replaced is no longer the app's, so the guest must stop
  // swallowing it too.
  const old = keydown("+", { ctrl: true });
  fire("keydown", old);
  assert.equal(old.defaultPrevented, false);
  assert.equal(shortcutPosts(posted).length, 1);
});

test("a malformed set_shortcuts leaves the guest forwarding nothing rather than throwing", () => {
  const { posted, send, fire } = loadGraphScript();
  send({ type: "set_shortcuts", combos: activeShortcutCombos({}) });
  send({ type: "set_shortcuts" });
  const e = keydown("-", { ctrl: true });
  fire("keydown", e);
  assert.equal(e.defaultPrevented, false);
  assert.deepEqual(shortcutPosts(posted), []);
});

// ── Filtered-to-nothing notice ──────────────────────────────────────────────
// The host's overlay keys off `graph_loaded.nodeCount`, i.e. what the BACKEND
// sent, so a graph the user has filtered down to nothing is still "ready" as
// far as GraphPage knows — it paints a blank rectangle (isolate) or a field of
// 8% ghosts, both indistinguishable from a failed load. This notice is the
// guest's own answer to that, and it has to stay quiet for the empty-library
// case the host already covers.

const noticeState = (doc: any) => ({
  shown: doc.getElementById("no-match-notice").style.display !== "none",
  title: doc.getElementById("no-match-title").textContent,
  body: doc.getElementById("no-match-body").textContent,
});

test("a filter that matches no paper raises the no-match notice", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  assert.equal(noticeState(document).shown, false);

  ctx.filterGraph({ highlight: "zzz-no-such-title" });
  const shown = noticeState(document);
  assert.equal(shown.shown, true);
  assert.equal(shown.title, "No matches");
  assert.equal(shown.body, "No papers match the active filters.");
  assert.equal(document.getElementById("no-match-notice").attrs.get("aria-hidden"), "false");
});

test("the no-match notice drops again as soon as something matches", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  ctx.filterGraph({ highlight: "zzz-no-such-title" });
  assert.equal(noticeState(document).shown, true);

  ctx.filterGraph({ highlight: "attention" });
  assert.equal(noticeState(document).shown, false);
  assert.equal(document.getElementById("no-match-notice").attrs.get("aria-hidden"), "true");
});

test("an empty library leaves the no-match notice to the host's EmptyState", () => {
  const { ctx, document } = loadGraphScript();
  // nodeCount 0 is what GraphPage turns into "Nothing to graph yet"; a second
  // "nothing here" panel on the same rectangle would be the guest's fault.
  ctx.loadGraph({ nodes: [], edges: [] }, {});
  assert.equal(noticeState(document).shown, false);
});

test("switching off every node type says so rather than blaming the filters", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});

  ctx.filterGraph({ showPapers: false, showAuthors: false, showTags: false });
  const shown = noticeState(document);
  assert.equal(shown.shown, true);
  assert.equal(shown.title, "Nothing to draw");
  assert.match(shown.body, /Visibility/);
});

test("hiding papers alone still draws the authors and tags, so no notice", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  ctx.filterGraph({ showPapers: false });
  assert.equal(noticeState(document).shown, false);
});

test("the notice's Clear all filters button resets the filters and dismisses it", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  document.getElementById("filterTitle").value = "zzz-no-such-title";
  ctx.filterGraph({ highlight: "zzz-no-such-title" });
  assert.equal(noticeState(document).shown, true);

  document.getElementById("no-match-clear").handlers.get("click")!();
  assert.equal(document.getElementById("filterTitle").value, "");
  assert.equal(noticeState(document).shown, false);
});

// ── Hidden single-paper authors, named by the notice ────────────────────────
// "Hide single-paper authors" is a checkbox in the HOST's page header applied
// by the backend (author_rows_sql joins author_paper_counts), so the authors it
// drops arrive as neither a node nor an edge — and the Filters › Author box
// matches through _paperAuthorLabels, which loadGraph builds from those edges.
// The filter is therefore blind to them, and the only symptom is an empty
// canvas under "No papers match the active filters".

const hintState = (doc: any) => ({
  hint: doc.getElementById("no-match-hint").style.display !== "none",
  button: doc.getElementById("no-match-show-authors").style.display !== "none",
  text: doc.getElementById("no-match-hint").textContent,
});

test("an Author filter that matches nothing names the hidden-authors option", () => {
  // ?excludeSingleAuthors=1 is what graphIframeSrc freezes into the src when
  // the host checkbox is on.
  const { ctx, document } = loadGraphScript({ search: "?excludeSingleAuthors=1" });
  ctx.loadGraph(PAYLOAD, {});

  ctx.filterGraph({ authorFilter: "hinton" });
  assert.equal(noticeState(document).shown, true);
  const shown = hintState(document);
  assert.equal(shown.hint, true);
  assert.match(shown.text, /single paper are hidden/);
  assert.equal(shown.button, true, "the option lives in the host, so offer to undo it");
});

test("the hidden-authors hint stays quiet when the option is off", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});

  ctx.filterGraph({ authorFilter: "hinton" });
  assert.equal(noticeState(document).shown, true, "still a no-match notice");
  assert.deepEqual(hintState(document), { hint: false, button: false, text: "" });
});

test("the hidden-authors hint stays quiet when the Author box is empty", () => {
  const { ctx, document } = loadGraphScript({ search: "?excludeSingleAuthors=1" });
  ctx.loadGraph(PAYLOAD, {});

  // Some other filter emptied the canvas; nothing about it involves authors, so
  // offering the authors option would be a guess dressed as an explanation.
  ctx.filterGraph({ highlight: "zzz-no-such-title" });
  assert.equal(noticeState(document).shown, true);
  assert.deepEqual(hintState(document), { hint: false, button: false, text: "" });
});

test("switching every node type off keeps its own single cause", () => {
  const { ctx, document } = loadGraphScript({ search: "?excludeSingleAuthors=1" });
  ctx.loadGraph(PAYLOAD, {});

  // The "Nothing to draw" body already names what did it; a second maybe-cause
  // beside it is noise.
  ctx.filterGraph({ authorFilter: "hinton", showPapers: false, showAuthors: false, showTags: false });
  assert.equal(noticeState(document).title, "Nothing to draw");
  assert.deepEqual(hintState(document), { hint: false, button: false, text: "" });
});

test("the hidden-authors hint is cleared once the filter matches again", () => {
  const { ctx, document } = loadGraphScript({ search: "?excludeSingleAuthors=1" });
  ctx.loadGraph(PAYLOAD, {});
  ctx.filterGraph({ authorFilter: "hinton" });
  assert.equal(hintState(document).hint, true);

  ctx.filterGraph({ authorFilter: "ada" });
  assert.equal(noticeState(document).shown, false);
  // The card is hidden, but a stale reason inside it is still state the next
  // show would have to overwrite.
  assert.deepEqual(hintState(document), { hint: false, button: false, text: "" });
});

test("the notice's Show single-paper authors button asks the host to clear the option", () => {
  const { ctx, document, posted } = loadGraphScript({ search: "?excludeSingleAuthors=1" });
  ctx.loadGraph(PAYLOAD, {});
  ctx.filterGraph({ authorFilter: "hinton" });

  document.getElementById("no-match-show-authors").handlers.get("click")!();
  // The guest never flips its own copy: GraphPage owns the checkbox and posts
  // the `set_options` reload back, so asking is the whole job. Asserted field
  // by field, not with deepEqual — the object is built inside the vm realm, so
  // its prototype is not this realm's Object.prototype.
  const asks = posted.filter((m: any) => m.type === "request_options");
  assert.equal(asks.length, 1);
  assert.equal(asks[0].excludeSingleAuthors, false);
});

test("the no-match notice is centred in the strip the panel column leaves free", () => {
  const { ctx, document } = loadGraphScript({ innerWidth: 1200, innerHeight: 800 });
  panelRect(document, 1200);
  const notice = document.getElementById("no-match-notice");
  // graph.css gives it width 280; a vm has no layout, so state it here.
  notice.getBoundingClientRect = () => ({ left: 0, right: 280, top: 0, bottom: 100, width: 280, height: 100 });
  ctx.loadGraph(PAYLOAD, {});

  ctx.filterGraph({ highlight: "zzz-no-such-title" });
  // panelRect puts the column's left edge 256px in from the right, leaving a
  // 944px strip: (944 - 280) / 2 = 332.
  assert.equal(notice.style.left, "332px");
});

// ── "Randomize & restart" (Layout panel) ────────────────────────────────────
// The button throws the settled layout away and re-seeds every node into a
// fresh 800x800 random square, which is the exact state a cold load is in.

/** Click the Layout panel's "Randomize & restart" button. */
function relayout(document: any) {
  document.getElementById("relayout-btn").handlers.get("click")!();
}

test("Randomize & restart reframes once the new layout settles", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  recorder.simHandlers.get("end")!();
  const framed = recorder.fits;

  relayout(document);
  // The viewport still frames the layout that was just discarded; the force
  // sim spreads the new seeds well past them, so without this the user clicks
  // "randomize" and lands on a stale, often near-empty, view.
  recorder.simHandlers.get("end")!();
  assert.equal(recorder.fits, framed + 1);

  // Still one-shot: filter changes and drags restart the sim too.
  recorder.simHandlers.get("end")!();
  assert.equal(recorder.fits, framed + 1);
});

test("grabbing a node after Randomize & restart cancels its reframe", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  recorder.simHandlers.get("end")!();
  const framed = recorder.fits;

  relayout(document);
  emitCy("grab", { id: () => "1", position: () => ({ x: 0, y: 0 }) }, "node");
  recorder.simHandlers.get("end")!();
  assert.equal(recorder.fits, framed, "must not yank the viewport out from under a drag");
});

test("Randomize & restart re-seeds every node's position", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  const before = new Map(
    [...ctx.__probe().simNodeById].map(([id, n]: any) => [id, { x: n.x, y: n.y }])
  );
  relayout(document);
  const after = ctx.__probe().simNodeById;
  let moved = 0;
  before.forEach((p: any, id: string) => {
    const n = after.get(id);
    if (n.x !== p.x || n.y !== p.y) moved++;
  });
  assert.equal(moved, before.size, "every node gets a fresh seed position");
});

test("Randomize & restart re-pins the nodes the active filter excludes", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  // Paper 2 is excluded; filterGraph pins it so it stops pushing the matching
  // half of the graph around.
  document.getElementById("filterTitle").value = "attention";
  ctx.filterGraph({ highlight: "attention" });
  const pinnedBefore = ctx.__probe().simNodeById.get("2");
  assert.notEqual(pinnedBefore.fx, null, "precondition: the filter pins what it excludes");

  relayout(document);

  // Randomizing clears fx/fy for every node — including the pins the filter
  // owns — so without re-applying the filter the excluded nodes drift under
  // the centring force as 8% ghosts, and _filterPinned is left true against a
  // null fx so the next filter pass cannot re-pin them either.
  const pinned = ctx.__probe().simNodeById.get("2");
  assert.equal(pinned.fx, pinned.x, "re-pinned at its NEW seed position");
  assert.equal(pinned.fy, pinned.y);
  assert.equal(pinned._filterPinned, true);

  // A node the filter kept stays free to be laid out.
  const free = ctx.__probe().simNodeById.get("1");
  assert.equal(free.fx, null);
  assert.equal(free._filterPinned, false);
});

test("Randomize & restart hides a hover inspector left pointing at a node", () => {
  const { ctx, document } = loadGraphScript({ innerWidth: 1200, innerHeight: 800 });
  ctx.loadGraph(PAYLOAD, {});
  emitCy("mouseover", recorder.cyNodeById.get("1"), "node");
  assert.equal(document.getElementById("node-tooltip").style.display, "");

  relayout(document);
  assert.equal(document.getElementById("node-tooltip").style.display, "none");
});

test("Randomize & restart with no graph loaded is a no-op", () => {
  const { document } = loadGraphScript();
  relayout(document);
  assert.equal(recorder.fits, 0);
});

// ── The Repel force slider vs. the filter's layout membership ───────────────
// filterGraph does not remove an excluded node from the simulation — it PINS it
// (fx/fy) and zeroes its charge — so the ghost stays put and stops pushing. A
// pin alone is not enough: d3's many-body force reads every node in the array
// whether or not it can move, so a pinned node with a live charge goes on
// shoving the matching half of the graph around from off-screen.

/** Drag the Layout panel's Repel force slider to `v`. */
function dragRepel(document: any, v: number) {
  const el = document.getElementById("repelForce");
  el.value = String(v);
  el.handlers.get("input")!();
}

/** The `strength` the simulation's charge force is currently holding. */
function chargeStrength() {
  return recorder.forces.get("charge")?.__strength;
}

/** That strength as it applies to one node, whichever form d3 was handed. */
function repulsionOf(id: string) {
  const s = chargeStrength();
  return typeof s === "function" ? s({ id }) : s;
}

test("the charge force skips the nodes the filter excluded from the layout", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  document.getElementById("filterTitle").value = "attention";
  ctx.filterGraph({ highlight: "attention" });

  const strength = chargeStrength();
  assert.equal(typeof strength, "function", "membership is per node, not a flat number");
  assert.equal(strength({ id: "1" }), -180, "a matching paper repels at the slider value");
  assert.equal(strength({ id: "2" }), 0, "an excluded paper exerts nothing");
});

test("dragging Repel force keeps the excluded nodes out of the layout", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  document.getElementById("filterTitle").value = "attention";
  ctx.filterGraph({ highlight: "attention" });

  dragRepel(document, 400);

  // The slider used to install a flat `strength(-v)`, which handed every
  // filtered-out node its full repulsion back for as long as the filter stayed
  // on — the ghosts still cannot move, so the visible graph was blown apart by
  // nodes the user had just filtered away, with no filter control touched to
  // explain it. The next filter pass quietly put it back, which made the
  // slider itself look unstable.
  const strength = chargeStrength();
  assert.equal(typeof strength, "function", "the new magnitude must not drop the membership");
  assert.equal(strength({ id: "1" }), -400, "the drag is what sets the magnitude");
  assert.equal(strength({ id: "2" }), 0);
});

test("dragging Repel force with no filter active repels every node equally", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  dragRepel(document, 250);

  // Read form-agnostically: with nothing filtered out every node is a member,
  // so a flat strength and a per-node one are the same answer — which is
  // exactly why the slider's flat form stayed invisible until a filter was on.
  // This is the guard that the membership fix did not narrow the unfiltered
  // case, not a second copy of the test above.
  ["1", "2", "author::7", "tag::ml"].forEach((id) => {
    assert.equal(repulsionOf(id), -250, id);
  });
});

/** The `radius` the simulation's collision force is currently holding. */
function collideRadius() {
  return recorder.forces.get("collision")?.__radius;
}

/** That radius as it applies to one node, whichever form d3 was handed. */
function collisionRadiusOf(id: string) {
  const r = collideRadius();
  return typeof r === "function" ? r({ id }) : r;
}

// ── The collision force vs. the filter's layout membership ─────────────────
// The other half of the same rule the charge force carries. Zeroing the charge
// stops an excluded node PULLING or PUSHING the layout, but d3's collision
// force reads every node in the array too, so a pinned ghost went on occupying
// a 28px keep-out circle and shoving the filtered-down graph around as it
// collapsed towards the origin under the centring force.

test("the collision force skips the nodes the filter excluded from the layout", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  document.getElementById("filterTitle").value = "attention";
  ctx.filterGraph({ highlight: "attention" });

  const radius = collideRadius();
  assert.equal(typeof radius, "function", "membership is per node, not a flat number");
  assert.equal(radius({ id: "1" }), 14, "a matching paper still keeps its neighbours off");
  assert.equal(radius({ id: "2" }), 0, "an excluded paper takes up no room");
  // A zero radius is exactly right, not merely smaller: d3 splits a collision
  // correction in proportion to the SQUARE of the pair's radii, so the member
  // takes 0 of it and the pinned ghost takes all of it and discards it.
  assert.equal(radius({ id: "author::7" }), 14, "an author the filter kept still does");
});

test("clearing the filter gives every node its collision radius back", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  document.getElementById("filterTitle").value = "attention";
  ctx.filterGraph({ highlight: "attention" });
  assert.equal(collisionRadiusOf("2"), 0, "precondition: paper 2 is out");

  document.getElementById("filterTitle").value = "";
  ctx.filterGraph({ highlight: "" });

  // Read form-agnostically for the same reason the Repel equivalent is: with
  // nothing excluded a flat radius and a per-node one are the same answer.
  ["1", "2", "author::7", "tag::ml"].forEach((id) => {
    assert.equal(collisionRadiusOf(id), 14, id);
  });
});

test("a cold load gives every node a collision radius", () => {
  const { ctx } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  ["1", "2", "author::7", "tag::ml"].forEach((id) => {
    assert.equal(collisionRadiusOf(id), 14, id);
  });
});

test("a reload drops the previous graph's layout membership", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  document.getElementById("filterTitle").value = "attention";
  ctx.filterGraph({ highlight: "attention" });
  assert.equal(chargeStrength()({ id: "2" }), 0, "precondition: paper 2 is out");

  // A Refresh rebuilds the simulation. The filter boxes survive it (the guest
  // keeps its panel state), so _applyFilter re-derives the membership — but it
  // must be re-derived from the NEW nodes, never carried over.
  document.getElementById("filterTitle").value = "";
  ctx.loadGraph(PAYLOAD, { preserveView: true });
  const strength = chargeStrength();
  assert.equal(strength({ id: "1" }), -180);
  assert.equal(strength({ id: "2" }), -180, "no filter now, so nothing is excluded");
});

// ── In-place reload: where a node that is new to the load starts ───────────
// A Refresh (or the "hide single-paper authors" toggle) reloads with
// preserveView, which reuses the settled layout and deliberately holds the
// viewport. A node that was not in the previous load has no position to reuse,
// and the cold-load seed is a random point in an 800x800 box at the world
// origin — nowhere near the neighbourhood a spread-out layout has put its
// authors and tags in, and a clump right on top of whatever else lives at the
// centre.

/** Push the settled layout far from the origin, as a real force layout does. */
function scatterSettledLayout(ctx: any, dx: number, dy: number) {
  ctx.__probe().simNodeById.forEach((sn: any) => { sn.x += dx; sn.y += dy; });
}

/** PAYLOAD plus the given nodes/edges, i.e. what a later /api/graph returns. */
function payloadPlus(nodes: unknown[], edges: unknown[]) {
  return { nodes: [...PAYLOAD.nodes, ...nodes], edges: [...PAYLOAD.edges, ...edges] };
}

const NEW_PAPER = {
  id: 3, source_id: "arxiv:2204.3", label: "Fresh import", type: "paper",
  category: "cs.LG", tags: ["ML"], has_pdf: true, published: "2024-03-01",
  url: null, doi: null, summary: null,
};

test("an in-place reload seeds a new paper beside the neighbours it is joined to", () => {
  const { ctx } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  scatterSettledLayout(ctx, 3000, -2500);
  const before = ctx.__probe().simNodeById;
  const author = { x: before.get("author::7").x, y: before.get("author::7").y };
  const tag    = { x: before.get("tag::ml").x,   y: before.get("tag::ml").y };

  // The paper the user just imported, joined to an author and a tag the
  // library already had.
  ctx.loadGraph(
    payloadPlus([NEW_PAPER], [{ source: 3, target: "author::7" }, { source: 3, target: "tag::ml" }]),
    { preserveView: true }
  );

  const seeded = ctx.__probe().simNodeById.get("3");
  // Centroid of the two placed neighbours, give or take the anti-stacking jitter.
  assert.ok(Math.abs(seeded.x - (author.x + tag.x) / 2) <= 20, `x ${seeded.x}`);
  assert.ok(Math.abs(seeded.y - (author.y + tag.y) / 2) <= 20, `y ${seeded.y}`);
});

test("an in-place reload places a new paper's brand-new author off the paper", () => {
  const { ctx } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  scatterSettledLayout(ctx, 3000, -2500);

  // The new author is joined to nothing that existed before, so it is only
  // reachable once the paper itself has been placed — the second seeding pass.
  ctx.loadGraph(
    payloadPlus(
      [NEW_PAPER, { id: "author::9", label: "Grace Hopper", type: "author", author_id: 9 }],
      [{ source: 3, target: "author::7" }, { source: 3, target: "author::9" }]
    ),
    { preserveView: true }
  );

  const paper = ctx.__probe().simNodeById.get("3");
  const author = ctx.__probe().simNodeById.get("author::9");
  assert.ok(Math.abs(author.x - paper.x) <= 20, `x ${author.x} vs ${paper.x}`);
  assert.ok(Math.abs(author.y - paper.y) <= 20, `y ${author.y} vs ${paper.y}`);
});

test("an in-place reload leaves an all-new island on the ordinary seed box", () => {
  const { ctx } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  scatterSettledLayout(ctx, 3000, -2500);

  // Nothing joins these two to the existing graph, so there is no placed
  // neighbour to sit beside and the random box is the honest answer.
  ctx.loadGraph(
    payloadPlus(
      [{ ...NEW_PAPER, tags: [] }, { id: "author::9", label: "Grace Hopper", type: "author", author_id: 9 }],
      [{ source: 3, target: "author::9" }]
    ),
    { preserveView: true }
  );

  ["3", "author::9"].forEach(id => {
    const sn = ctx.__probe().simNodeById.get(id);
    assert.ok(Math.abs(sn.x) <= 400 && Math.abs(sn.y) <= 400, `${id} at ${sn.x},${sn.y}`);
  });
});

test("a reload that adds nothing leaves every surviving position untouched", () => {
  const { ctx } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  scatterSettledLayout(ctx, 3000, -2500);
  const before = new Map<string, { x: number; y: number }>();
  ctx.__probe().simNodeById.forEach((sn: any, id: string) => before.set(id, { x: sn.x, y: sn.y }));

  ctx.loadGraph(PAYLOAD, { preserveView: true });

  ctx.__probe().simNodeById.forEach((sn: any, id: string) => {
    assert.equal(sn.x, before.get(id)!.x, id);
    assert.equal(sn.y, before.get(id)!.y, id);
  });
});

test("a cold load still seeds every node in the random box", () => {
  const { ctx } = loadGraphScript();
  // No previous layout to sit beside: the neighbour seeding must stay out of
  // the way entirely, including for the LAYOUT_SEED demo path, whose rand
  // sequence it would otherwise consume.
  ctx.loadGraph(payloadPlus([NEW_PAPER], [{ source: 3, target: "author::7" }]), {});
  ctx.__probe().simNodeById.forEach((sn: any, id: string) => {
    assert.ok(Math.abs(sn.x) <= 400 && Math.abs(sn.y) <= 400, `${id} at ${sn.x},${sn.y}`);
  });
});

// ── Projects / Project Tags filter rows ─────────────────────────────────────

// One project or project-tag filter row as graph.js drew it. Each row is a
// [swatch slot, label, remove button] triple built with createElement, so the
// only way to read it back is through the recorded children.
function filterRows(document: any, containerId: string) {
  return document.getElementById(containerId).children.map((row: any) => {
    const slot = row.children[0];
    const dots = slot.children.filter((c: any) => c.className === "proj-swatch");
    const more = slot.children.find((c: any) => c.className === "proj-swatch-more");
    return {
      unmatched: row.className.split(" ").includes("unmatched"),
      colors: dots.map((d: any) => d.style.backgroundColor),
      more: more ? more.textContent : null,
      slotText: slot.textContent,
      slotTitle: slot.title,
      label: row.children[1].textContent,
    };
  });
}

function addProjectRow(ctx: any, document: any, value: string) {
  document.getElementById("filterProject").value = value;
  ctx._addToFilterList("filterProject", ctx.__probe().projectFilterNames, ctx._renderProjectRows);
}

function addProjTagRow(ctx: any, document: any, value: string) {
  document.getElementById("filterProjectTag").value = value;
  ctx._addToFilterList("filterProjectTag", ctx.__probe().projTagFilterNames, ctx._renderProjTagRows);
}

const PROJECTS = [
  { id: 5, name: "Thesis", color: "#ff0000", tags: ["ML"] },
  { id: 6, name: "Thesis appendix", color: "#00ff00", tags: ["MLops"] },
  { id: 7, name: "Reading", color: "#0000ff", tags: ["ml", "misc"] },
];

test("a Projects filter row wears the colour of every project it resolves to", () => {
  const { ctx, document } = loadGraphScript();
  ctx.setFilterOptions(null, null, PROJECTS);
  addProjectRow(ctx, document, "thesis");

  const rows = filterRows(document, "project-filter-rows");
  assert.equal(rows.length, 1);
  assert.equal(rows[0].label, "thesis");
  assert.equal(rows[0].unmatched, false);
  // Substring, case-insensitive — the same match _projectIdsFromInput makes.
  assert.deepEqual(rows[0].colors, ["#ff0000", "#00ff00"]);
  assert.equal(rows[0].slotTitle, "Thesis, Thesis appendix");
  // The dots are not a second opinion: they are the ids the canvas is filtered by.
  assert.deepEqual([...ctx._projectIdsFromInput()], [5, 6]);
});

test("a Projects filter row that resolves to nothing is marked as such", () => {
  const { ctx, document } = loadGraphScript();
  ctx.setFilterOptions(null, null, PROJECTS);
  addProjectRow(ctx, document, "thesos");

  const rows = filterRows(document, "project-filter-rows");
  assert.equal(rows[0].unmatched, true);
  assert.deepEqual(rows[0].colors, []);
  assert.equal(rows[0].slotText, "\u2717");
  // The reason it has to be visible: this row alone empties the canvas.
  assert.deepEqual([...ctx._projectIdsFromInput()], [-1]);
});

test("a Projects row matching more than three projects collapses the rest into a +n", () => {
  const { ctx, document } = loadGraphScript();
  ctx.setFilterOptions(null, null, [1, 2, 3, 4, 5].map(i => ({
    id: i, name: `Project ${i}`, color: `#00000${i}`, tags: [],
  })));
  addProjectRow(ctx, document, "project");

  const rows = filterRows(document, "project-filter-rows");
  assert.deepEqual(rows[0].colors, ["#000001", "#000002", "#000003"]);
  assert.equal(rows[0].more, "+2");
  // The cap is a display cap only — every match still filters.
  assert.deepEqual([...ctx._projectIdsFromInput()], [1, 2, 3, 4, 5]);
});

test("a Project Tags row resolves whole tags case-insensitively, never by substring", () => {
  const { ctx, document } = loadGraphScript();
  ctx.setFilterOptions(null, null, PROJECTS);
  addProjTagRow(ctx, document, "ml");

  const rows = filterRows(document, "proj-tag-filter-rows");
  // "MLops" contains "ml" but filterGraph compares project tags whole
  // (projTagSet.has), so the row must not claim it.
  assert.deepEqual(rows[0].colors, ["#ff0000", "#0000ff"]);
  assert.equal(rows[0].slotTitle, "Thesis, Reading");

  addProjTagRow(ctx, document, "nope");
  assert.equal(filterRows(document, "proj-tag-filter-rows")[1].unmatched, true);
});

// A reading list IS a project carrying the reserved READING_LIST_TAG
// (src/lib/readingStatus.ts), so `/api/graph/project-options` -- which forwards
// each project's project_tags raw -- carries the marker on every reading list.
const READING_LIST_PROJECTS = [
  { id: 8, name: "To read", color: "#abcdef", tags: ["reading-list", "queue"] },
  { id: 9, name: "Shortlist", color: "#fedcba", tags: ["Reading-List"] },
];

/** The option values of a datalist, as graph.js wrote them. */
function datalistValues(document: any, id: string): string[] {
  return [...document.getElementById(id).innerHTML.matchAll(/value="([^"]*)"/g)]
    .map((m: any) => m[1]);
}

test("the Project Tags dropdown does not offer the reserved reading-list marker", () => {
  const { ctx, document } = loadGraphScript();
  ctx.setFilterOptions(null, null, READING_LIST_PROJECTS);

  // Every other surface that draws a project's tags filters this one out
  // (ProjectDetailPage's chip row, ReadingListsPage's ProjectCards) and
  // `list_tags` keeps it out of /api/tags, so the graph was the last dropdown
  // in the app naming an implementation detail as if it were a user tag. The
  // project's real tag is untouched.
  assert.deepEqual(datalistValues(document, "projectTagList"), ["queue"]);
  // The Projects dropdown is unaffected: a reading list is a project, and
  // ProjectsPage lists it like any other.
  assert.deepEqual(datalistValues(document, "projectList"), ["To read", "Shortlist"]);
});

test("the marker is recognised whatever case the project stored it in", () => {
  const { ctx, document } = loadGraphScript();
  // PROJECT_TAG labels keep the casing they were written with, and
  // isReadingListProject folds before comparing; "Reading-List" is the same
  // marker and must not slip into the list through a capital.
  ctx.setFilterOptions(null, null, [{ id: 9, name: "Shortlist", tags: ["Reading-List"] }]);
  assert.deepEqual(datalistValues(document, "projectTagList"), []);
});

test("hiding the marker from the dropdown does not stop it filtering", () => {
  const { ctx, document } = loadGraphScript();
  ctx.setFilterOptions(null, null, READING_LIST_PROJECTS);
  // Only the OFFER is filtered -- _projectMap keeps every tag -- so a row typed
  // by hand still resolves and is not falsely marked unmatched. Same split
  // ProjectDetailPage makes: the chip is hidden, the tag still defines the list.
  addProjTagRow(ctx, document, "reading-list");
  const rows = filterRows(document, "proj-tag-filter-rows");
  assert.equal(rows[0].unmatched, false);
  assert.deepEqual(rows[0].colors, ["#abcdef", "#fedcba"]);
});

test("a project with no colour in the payload falls back to the accent", () => {
  const { ctx, document } = loadGraphScript();
  ctx.setFilterOptions(null, null, [{ id: 5, name: "Thesis", tags: [] }]);
  addProjectRow(ctx, document, "thesis");
  assert.deepEqual(filterRows(document, "project-filter-rows")[0].colors,
                   ["var(--color-accent, #5b8dee)"]);
});

test("reloaded project options repaint the rows drawn against the previous load", () => {
  const { ctx, document } = loadGraphScript();
  ctx.setFilterOptions(null, null, PROJECTS);
  addProjectRow(ctx, document, "reading");
  assert.deepEqual(filterRows(document, "project-filter-rows")[0].colors, ["#0000ff"]);

  // Recoloured elsewhere in the app, then Refresh: the row is free text the
  // user typed, but what it stands for is not, and it just changed.
  ctx.setFilterOptions(null, null, [{ id: 7, name: "Reading", color: "#123456", tags: [] }]);
  assert.deepEqual(filterRows(document, "project-filter-rows")[0].colors, ["#123456"]);

  // Deleted elsewhere: the row now filters the canvas down to nothing and says so.
  ctx.setFilterOptions(null, null, []);
  assert.equal(filterRows(document, "project-filter-rows")[0].unmatched, true);
});

test("a failed project-options request leaves the rows on their last known projects", async () => {
  const { ctx, document } = loadGraphScript();
  ctx.fetch = fetchServing({
    ...API_RESPONSES,
    // A paper in project 7, so the row below is one that genuinely filters —
    // the canvas-scoped marking is a separate rule with its own tests, and this
    // one is about the OPTIONS request failing.
    "/api/graph": { nodes: [{ ...PAYLOAD.nodes[0], project_ids: [7] }], edges: [] },
    "/api/graph/project-options": { projects: PROJECTS },
  });
  await ctx.fetchAndLoadGraph({ preserveView: false });
  addProjectRow(ctx, document, "reading");
  assert.deepEqual(filterRows(document, "project-filter-rows")[0].colors, ["#0000ff"]);

  // Same reason setFilterOptions keeps the datalist: null is "that request
  // failed", never "the library has no projects any more" — marking every row
  // unmatched would be a claim the failure does not support.
  ctx.setFilterOptions(null, null, null);
  assert.equal(filterRows(document, "project-filter-rows")[0].unmatched, false);
});

test("removing a filter row redraws the list instead of leaving it on screen", () => {
  const { ctx, document } = loadGraphScript();
  ctx.setFilterOptions(null, null, PROJECTS);
  addProjectRow(ctx, document, "thesis");
  addProjectRow(ctx, document, "reading");
  assert.equal(filterRows(document, "project-filter-rows").length, 2);

  // The row's own X button — the rerender it is handed has to be the one that
  // knows how to resolve this list, not a bare redraw of the container.
  const removeBtn = document.getElementById("project-filter-rows").children[0].children[2];
  removeBtn.handlers.get("click")();

  const rows = filterRows(document, "project-filter-rows");
  assert.deepEqual(rows.map((r: any) => r.label), ["reading"]);
  assert.deepEqual(rows[0].colors, ["#0000ff"]);
});

// ── A Projects / Project Tags row that stands for nothing on the canvas ─────
// `/api/graph/project-options` answers with every ACTIVE project, but both
// boxes filter PAPERS -- filterGraph tests a paper's own `project_ids` -- so a
// project with no paper on this canvas resolves perfectly, wears its colour
// swatch, and can only empty the graph. The Paper Tags list has checked its
// rows against the PAYLOAD (_paperTagSet) since the dropdown narrowing; these
// two siblings were still checking against the options answer alone.

/** A graph payload whose papers belong to the given projects. */
function payloadInProjects(...projectIdsPerPaper: number[][]) {
  return {
    nodes: projectIdsPerPaper.map((ids, i) => ({
      id: i + 1, source_id: `arxiv:${i + 1}`, label: `Paper ${i + 1}`, type: "paper",
      tags: [], project_ids: ids,
    })),
    edges: [],
  };
}

test("the Projects dropdown only offers projects a paper on the canvas belongs to", () => {
  const { ctx, document } = loadGraphScript();
  ctx.setFilterOptions(null, null, PROJECTS);
  // Every one of the three is active, so the options payload names all three;
  // only project 5 holds a paper this graph drew.
  ctx.loadGraph(payloadInProjects([5], []), {});

  assert.deepEqual(datalistValues(document, "projectList"), ["Thesis"]);
  // The Project Tags offer follows the same narrowing: "MLops" (project 6) and
  // "misc" (project 7) stand for projects with nothing on this canvas.
  assert.deepEqual(datalistValues(document, "projectTagList"), ["ML"]);
});

test("a Projects row whose project has no paper on this graph is marked as such", () => {
  const { ctx, document } = loadGraphScript();
  ctx.setFilterOptions(null, null, PROJECTS);
  ctx.loadGraph(payloadInProjects([5]), {});
  addProjectRow(ctx, document, "reading");

  const row = filterRows(document, "project-filter-rows")[0];
  assert.equal(row.unmatched, true);
  // The project is real and the swatch says so -- that is what tells this case
  // apart from a typo, which draws the slot's own "✗" and no dots.
  assert.deepEqual(row.colors, ["#0000ff"]);
  assert.equal(document.getElementById("project-filter-rows").children[0].title,
               "No paper on this graph belongs to that project.");
  // And the reason it has to be visible: this row alone empties the canvas.
  ctx.filterGraph({ projectIds: ctx._projectIdsFromInput() });
  assert.deepEqual([...ctx.__probe().visiblePaperIds], []);
});

test("a Project Tags row whose projects draw no paper is marked too", () => {
  const { ctx, document } = loadGraphScript();
  ctx.setFilterOptions(null, null, PROJECTS);
  ctx.loadGraph(payloadInProjects([5]), {});
  // "misc" is project 7's tag alone, and project 7 has no paper here.
  addProjTagRow(ctx, document, "misc");
  const row = filterRows(document, "proj-tag-filter-rows")[0];
  assert.equal(row.unmatched, true);
  assert.deepEqual(row.colors, ["#0000ff"]);
  assert.equal(document.getElementById("proj-tag-filter-rows").children[0].title,
               "No paper on this graph belongs to a project with this tag.");
});

test("a row is kept whole as long as ONE of the projects it names draws a paper", () => {
  const { ctx, document } = loadGraphScript();
  ctx.setFilterOptions(null, null, PROJECTS);
  ctx.loadGraph(payloadInProjects([6]), {});
  // "thesis" resolves to 5 (no papers here) and 6 (one), and filterGraph ORs
  // the ids, so the row does filter -- marking it would be wrong.
  addProjectRow(ctx, document, "thesis");
  const row = filterRows(document, "project-filter-rows")[0];
  assert.equal(row.unmatched, false);
  assert.equal(document.getElementById("project-filter-rows").children[0].title, "");
});

test("the narrowing withholds the offer without narrowing what a typed row matches", () => {
  const { ctx, document } = loadGraphScript();
  ctx.setFilterOptions(null, null, PROJECTS);
  ctx.loadGraph(payloadInProjects([5]), {});
  assert.equal(datalistValues(document, "projectList").includes("Reading"), false);

  // Same split the reading-list marker gets: _projectMap keeps every project,
  // so a name typed by hand still resolves to its ids and still filters.
  addProjectRow(ctx, document, "reading");
  assert.deepEqual([...ctx._projectIdsFromInput()], [7]);
});

test("nothing is withheld before a payload has arrived", () => {
  const { ctx, document } = loadGraphScript();
  ctx.setFilterOptions(null, null, PROJECTS);
  // No load yet: claiming a project has no paper on a graph that has not
  // arrived would be a guess, so the offer is the whole options answer and a
  // row is judged on whether it resolves at all.
  assert.deepEqual(datalistValues(document, "projectList"),
                   ["Thesis", "Thesis appendix", "Reading"]);
  addProjectRow(ctx, document, "reading");
  assert.equal(filterRows(document, "project-filter-rows")[0].unmatched, false);
});

test("a reload stops offering a project whose last paper left the graph", () => {
  const { ctx, document } = loadGraphScript();
  ctx.setFilterOptions(null, null, PROJECTS);
  ctx.loadGraph(payloadInProjects([7]), {});
  assert.deepEqual(datalistValues(document, "projectList"), ["Reading"]);
  addProjectRow(ctx, document, "reading");
  assert.equal(filterRows(document, "project-filter-rows")[0].unmatched, false);

  // The paper is deleted elsewhere and the user hits Refresh. The PROJECT is
  // still active, so the options answer is unchanged -- only the payload knows,
  // and it arrives after setFilterOptions has already run.
  ctx.loadGraph(payloadInProjects([]), {});
  assert.deepEqual(datalistValues(document, "projectList"), []);
  assert.equal(filterRows(document, "project-filter-rows")[0].unmatched, true);
});

// ── Dragging a ghost vs. the filter's layout pins ──────────────────────────
// An excluded node is dimmed to 8%, not hidden, and `_eventsFor` only takes an
// element out of hit-testing at opacity 0 (isolate mode), so every ghost on the
// canvas is grabbable. filterGraph pins what it excludes; the `free` handler
// used to null fx/fy for whatever was dropped, which released that pin and left
// the ghost — charge 0, collision radius 0 — drifting towards the origin under
// the centring force until the next filter pass happened to re-pin it.

/** The `fx` the simulation node for `id` is currently holding. */
function ghostFx(ctx: any, id: string) {
  return ctx.__probe().simNodeById.get(id).fx;
}

/** Grab a node, drag it to (x, y), drop it — the three events cytoscape fires. */
function dragNode(id: string, x: number, y: number) {
  const target = { id: () => id, position: () => ({ x, y }) };
  emitCy("grab", recorder.cyNodeById.get(id), "node");
  emitCy("drag", target, "node");
  emitCy("free", recorder.cyNodeById.get(id), "node");
}

test("dropping a node the filter excluded leaves the filter's pin in place", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  document.getElementById("filterTitle").value = "attention";
  ctx.filterGraph({ highlight: "attention" });
  const ghost = ctx.__probe().simNodeById.get("2");
  assert.equal(ghost._filterPinned, true, "precondition: the filter pins what it excludes");

  dragNode("2", 500, 400);

  assert.equal(ghost.fx, 500, "stays where the user dropped it");
  assert.equal(ghost.fy, 400);
  assert.equal(ghost._filterPinned, true, "the pin is still the filter's to release");
});

test("dropping a node the filter kept hands it back to the layout", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  document.getElementById("filterTitle").value = "attention";
  ctx.filterGraph({ highlight: "attention" });

  dragNode("1", 500, 400);

  // A matching node is a full member of the layout, so releasing it must go on
  // meaning "let the simulation place this again" — the fix must not freeze
  // every dropped node.
  assert.equal(ghostFx(ctx, "1"), null);
  assert.equal(ctx.__probe().simNodeById.get("1")._filterPinned, false);
});

test("re-admitting a dropped ghost releases the pin it was left with", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  document.getElementById("filterTitle").value = "attention";
  ctx.filterGraph({ highlight: "attention" });
  dragNode("2", 500, 400);
  assert.equal(ghostFx(ctx, "2"), 500, "precondition: the drop left it pinned");

  document.getElementById("filterTitle").value = "";
  ctx.filterGraph({ highlight: "" });

  // _filterPinned is what makes the re-pin releasable: leaving fx set without
  // it would freeze the node at the drop point for the rest of the session,
  // with no filter active to explain why it never moves again.
  const readmitted = ctx.__probe().simNodeById.get("2");
  assert.equal(readmitted.fx, null);
  assert.equal(readmitted._filterPinned, false);
});

test("dropping a node with no filter active hands it back to the layout", () => {
  const { ctx } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});

  // The unfiltered case is where the old and new handlers agree — every node is
  // a layout member — so this is the guard that the fix did not narrow ordinary
  // dragging, not a second copy of the tests above.
  ["1", "2", "author::7", "tag::ml"].forEach((id) => {
    dragNode(id, 500, 400);
    const sn = ctx.__probe().simNodeById.get(id);
    assert.equal(sn.fx, null, id);
    assert.equal(sn._filterPinned, false, id);
  });
});

// ── Active-filter badges ────────────────────────────────────────────────────
// The Filters and Tag Filter panels open COLLAPSED (graph.html) and their state
// outlives every navigation, Refresh and option toggle -- AppShell keeps this
// iframe alive across route changes. A partially filtered canvas is a field of
// 8% ghosts, which is what an active filter is supposed to look like, and the
// no-match notice only appears once NOTHING is drawn, so until now the whole
// partially-filtered range had no cue anywhere on the two collapsed headers.

/** The count badge on a panel header: its text and the lines it names. */
function panelCount(document: any, id: string) {
  const el = document.getElementById(id);
  return { text: el.textContent, lines: el.title ? el.title.split("\n") : [] };
}

test("an untouched graph leaves both panel headers exactly as they read before", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});

  // loadGraph ends in a filter pass, so the badges have been through the paint
  // path — "nothing active" has to survive it as empty rather than "(0)".
  assert.deepEqual(panelCount(document, "filter-active-count"), { text: "", lines: [] });
  assert.deepEqual(panelCount(document, "tag-filter-active-count"), { text: "", lines: [] });
});

test("the Filters header counts what a collapsed panel is hiding, and names it", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});

  document.getElementById("filterCategory").value = " cs.LG ";
  document.getElementById("filterHasPdf").checked = true;
  document.getElementById("showAuthors").checked = false;
  document.getElementById("filterDateFrom").value = "2024-01-01";
  document.getElementById("filterAuthor").value = "Hinton";
  ctx._applyFilter();

  const badge = panelCount(document, "filter-active-count");
  assert.equal(badge.text, " (5)");
  assert.deepEqual(badge.lines, [
    "Authors hidden",
    "Category: cs.LG",   // trimmed, the same value _applyFilter passes on
    "Has PDF only",
    "Published from 2024-01-01",
    "Author: Hinton",
  ]);
  // Whitespace alone is not a filter — _applyFilter trims to null too.
  document.getElementById("filterCategory").value = "   ";
  ctx._applyFilter();
  assert.equal(panelCount(document, "filter-active-count").text, " (4)");
});

test("the isolate toggle counts as an active filter", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  // "Show highlighted only" is a class on a button, not an input value, and it
  // is the one Filters control that leaves the canvas BLANK rather than dim.
  document.getElementById("isolate-btn").classList = {
    contains: (c: string) => c === "active",
    add() {}, remove() {}, toggle() {},
  };
  ctx._applyFilter();

  const badge = panelCount(document, "filter-active-count");
  assert.equal(badge.text, " (1)");
  assert.deepEqual(badge.lines, ["Show highlighted only"]);
});

test("the Tag Filter header counts rows, not text left sitting in an add box", () => {
  const { ctx, document } = loadGraphScript();
  ctx.setFilterOptions(null, null, PROJECTS);
  ctx.loadGraph(PAYLOAD, {});

  // Typed but never added: nothing reads it until "+"/Enter, so it is not a
  // filter and must not be counted as one.
  document.getElementById("tagFilterInput").value = "half-typed";
  ctx._applyFilter();
  assert.equal(panelCount(document, "tag-filter-active-count").text, "");

  addTagRow(ctx, document, "ML");
  addTagRow(ctx, document, "nlp");
  addProjectRow(ctx, document, "thesis");
  addProjTagRow(ctx, document, "ml");

  const badge = panelCount(document, "tag-filter-active-count");
  assert.equal(badge.text, " (4)");
  // The AND/OR a paper-tag row is combined with rides along: two rows joined by
  // OR filter nothing like the same two joined by AND.
  assert.deepEqual(badge.lines, [
    "Project: thesis",
    "Project tag: ml",
    "Tag: ML",
    "AND Tag: nlp",
  ]);
  // The Filters panel is a separate count and stays quiet.
  assert.equal(panelCount(document, "filter-active-count").text, "");
});

test("Clear all filters empties both badges and every add box", () => {
  const { ctx, document } = loadGraphScript();
  ctx.setFilterOptions(null, null, PROJECTS);
  ctx.loadGraph(PAYLOAD, {});

  document.getElementById("filterTitle").value = "attention";
  document.getElementById("showTags").checked = false;
  addTagRow(ctx, document, "ML");
  addProjectRow(ctx, document, "thesis");
  document.getElementById("filterProject").value = "half-typed";
  document.getElementById("filterProjectTag").value = "half-typed";
  document.getElementById("tagFilterInput").value = "half-typed";
  ctx._applyFilter();
  assert.equal(panelCount(document, "filter-active-count").text, " (2)");
  assert.equal(panelCount(document, "tag-filter-active-count").text, " (2)");

  ctx.clearFilters();

  assert.deepEqual(panelCount(document, "filter-active-count"), { text: "", lines: [] });
  assert.deepEqual(panelCount(document, "tag-filter-active-count"), { text: "", lines: [] });
  // All three sibling add-boxes, not two: leaving one holding half-typed text
  // after "Clear all filters" is the panel disagreeing with its own button.
  ["filterProject", "filterProjectTag", "tagFilterInput"].forEach((id) => {
    assert.equal(document.getElementById(id).value, "", id);
  });
});

test("a reload keeps the badges in step with the filters it re-applies", () => {
  const { ctx, document } = loadGraphScript();
  ctx.loadGraph(PAYLOAD, {});
  document.getElementById("filterCategory").value = "cs.LG";
  addTagRow(ctx, document, "ML");
  assert.equal(panelCount(document, "filter-active-count").text, " (1)");

  // Refresh / "hide single-paper authors" reload. The filters survive it (the
  // frame is never torn down), so the badges have to survive it too — loadGraph
  // ends in _applyFilter, which is the single funnel they are painted from.
  ctx.loadGraph(PAYLOAD, { preserveView: true });
  assert.equal(panelCount(document, "filter-active-count").text, " (1)");
  assert.equal(panelCount(document, "tag-filter-active-count").text, " (1)");
});

// ── Tag chips are spelled the way the rest of the app spells them ───────────
// A tag NODE is shared by every paper carrying the tag, but `/api/graph` labels
// it from the papers rather than from the TAG table: crates/core/src/graph.rs
// keys it `tag::<lower>` and keeps the casing of whichever paper it reached
// first (`or_insert_with` over PAPER_NODES_SQL, which has no ORDER BY). So one
// tag written "ML" by the Tags page and "ml" by an imported archive drew a chip
// whose spelling depended on SQLite's scan order — it could change under a
// plain Refresh, after an unrelated edit moved the rows — while the Paper Tags
// dropdown two panels away offered "ML" and TagPage titled itself "ML".

/** The label graph.js actually handed cytoscape for a node. */
function drawnLabel(id: string): string {
  return recorder.cyNodeById.get(id).data("label");
}

/** A canvas whose one tag node the payload spelled `spelling`. */
function taggedPayload(spelling: string) {
  return {
    nodes: [
      { id: 1, source_id: "arxiv:1", label: "Attention", type: "paper", category: null, tags: [spelling], has_pdf: false, published: null, url: null, doi: null, summary: null },
      { id: `tag::${spelling.toLowerCase()}`, label: spelling, type: "tag" },
    ],
    edges: [{ source: 1, target: `tag::${spelling.toLowerCase()}` }],
  };
}

test("a tag chip carries the TAG table's spelling, not the first paper's", () => {
  const { ctx } = loadGraphScript();
  // fetchAndLoadGraph's own order: the options are installed, then the payload
  // is loaded against them.
  ctx.setFilterOptions(null, ["ML"], null);
  ctx.loadGraph(taggedPayload("ml"), {});

  assert.equal(drawnLabel("tag::ml"), "ML");
});

test("the chip follows the TAG table even when the payload shouts louder", () => {
  const { ctx } = loadGraphScript();
  // The other direction, so the rule cannot be read as "prefer the louder
  // casing": whatever `/api/tags` answers with is the spelling, because that is
  // the one the Tags index, TagPage and the Paper Tags dropdown all show.
  ctx.setFilterOptions(null, ["ml"], null);
  ctx.loadGraph(taggedPayload("ML"), {});

  assert.equal(drawnLabel("tag::ml"), "ml");
});

test("the inspector spells a tag the way the chip beside it does", () => {
  const { ctx, document } = loadGraphScript();
  ctx.setFilterOptions(null, ["ML"], null);
  ctx.loadGraph(taggedPayload("ml"), {});

  // The tag line names what the canvas DREW, so it cannot go on reading the
  // paper's own casing once the chip stopped using it.
  hover("1");
  const meta = document.getElementById("node-tooltip-meta").textContent as string;
  assert.ok(meta.split("\n").includes("ML"), meta);
});

test("tapping a relabelled tag hands the host the canonical spelling", () => {
  const { ctx, posted } = loadGraphScript();
  ctx.setFilterOptions(null, ["ML"], null);
  ctx.loadGraph(taggedPayload("ml"), {});

  emitCy("tap", recorder.cyNodeById.get("tag::ml"), 'node[type = "tag"]');

  const msg = posted.filter((m: any) => m.type === "tag_clicked").pop();
  assert.equal(msg.label, "ML");
});

test("a tag /api/tags does not name keeps the label the payload drew it with", () => {
  const { ctx } = loadGraphScript();
  // `list_tags_with_count` filters the reserved reading-list marker out of
  // /api/tags server-side, so a paper carrying it has a chip the list cannot
  // speak for. Falling back to the payload's label is the only honest answer —
  // the alternative is a chip with no text at all.
  ctx.setFilterOptions(null, ["ML"], null);
  ctx.loadGraph(taggedPayload("Reading-List"), {});

  assert.equal(drawnLabel("tag::reading-list"), "Reading-List");
});

test("before /api/tags answers the payload's own label stands", () => {
  const { ctx } = loadGraphScript();
  // A cold load whose dropdown request failed (or has not resolved): there is
  // nothing to resolve against, and inventing a spelling would be a guess.
  ctx.loadGraph(taggedPayload("ml"), {});

  assert.equal(drawnLabel("tag::ml"), "ml");
});
