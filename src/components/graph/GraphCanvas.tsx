import { forwardRef, useCallback, useEffect, useImperativeHandle, useRef, useState } from "react";
import cytoscape from "cytoscape";
import type { Core, NodeSingular } from "cytoscape";
import {
  forceCollide,
  forceLink,
  forceManyBody,
  forceSimulation,
  forceX,
  forceY,
} from "d3-force";
import type { ForceLink, Simulation, SimulationLinkDatum, SimulationNodeDatum } from "d3-force";

import type { ThemeColors } from "../../lib/theme";
import type { GraphIndex, GraphNodeType, GraphView } from "../../lib/graph/model";
import type { GraphMatch } from "../../lib/graph/filter";
import { layoutIds } from "../../lib/graph/filter";
import type { ForceSettings } from "../../lib/graph/layout";
import { layoutRng, randomizePositions, seedPositions } from "../../lib/graph/layout";
import { fitViewport, placeFloatingBox, FIT_PADDING } from "../../lib/graph/fit";
import {
  eventsFor,
  graphStylesheet,
  highlightColor,
  MAX_ZOOM,
  MIN_ZOOM,
  opacityFor,
  paperColor,
  whenLabelFontReady,
} from "../../lib/graph/style";
import { tooltipFor } from "../../lib/graph/tooltip";
import type { TooltipContent } from "../../lib/graph/tooltip";
import { MathText } from "../../lib/tex";

/**
 * The graph engine: one cytoscape instance for drawing and one d3-force
 * simulation for placing, wired together by a per-tick position sync.
 *
 * This is the half of the old `public/graph/graph.js` that genuinely has to be
 * imperative — two libraries that own mutable state and expect to be driven, not
 * re-rendered. Everything around it (the panels, the filter state, the loading
 * and empty screens, the selection) is ordinary React now, and everything the
 * DATABASE knows is resolved in Rust before the payload is sent. What is left
 * here is the canvas.
 */

/** A node as d3 holds it, plus the bookkeeping this component layers on. */
interface SimNode extends SimulationNodeDatum {
  id: string;
  x: number;
  y: number;
  /**
   * True when the pin under `fx`/`fy` is the FILTER's, not a drag's. The same
   * two slots have two writers and the release rules differ: a filter pin is
   * released when the node re-enters the layout, a drag pin when the user lets
   * go — and neither may clear the other's.
   */
  filterPinned?: boolean;
  cyNode?: NodeSingular;
}

type SimLink = SimulationLinkDatum<SimNode>;

export interface GraphCanvasHandle {
  /** Throw the settled layout away and rebuild it from fresh seed positions. */
  relayout(): void;
}

export interface GraphCanvasProps {
  view: GraphView;
  index: GraphIndex;
  theme: ThemeColors;
  forces: ForceSettings;
  match: GraphMatch;
  selectedIds: ReadonlySet<string>;
  /** Canvas pixels covered by the panel column, so a fit frames the visible strip. */
  gutter: number;
  /**
   * The same number read straight from the DOM.
   *
   * `gutter` is React state, and a fit can run in the same task as the
   * ResizeObserver callback that recomputes it — the reveal after this page was
   * kept alive behind `display: none` is exactly that moment. The `setGutter`
   * from that callback has not committed yet, so a fit reading the prop sees the
   * hidden column's 0 and frames the whole canvas, putting the rightmost nodes
   * under the panels for the rest of the session. A fit is about the DOM as it
   * is right now, so it measures rather than remembers.
   */
  measureGutter: () => number;
  onPaperTap: (id: string, additive: boolean) => void;
  onAuthorTap: (authorId: number) => void;
  onTagTap: (label: string) => void;
  onBackgroundTap: () => void;
}

interface TooltipState extends TooltipContent {
  left: number;
  top: number;
}

/** Per-node collision radius, so the layout holds two node centres 28px apart.
 *  Node bodies are 20px across (14px for an author diamond), which leaves a
 *  little room for the label that hangs off each one's right-hand side. */
const COLLIDE_RADIUS = 14;

const GraphCanvas = forwardRef<GraphCanvasHandle, GraphCanvasProps>(function GraphCanvas(
  {
    view,
    index,
    theme,
    forces,
    match,
    selectedIds,
    gutter,
    measureGutter,
    onPaperTap,
    onAuthorTap,
    onTagTap,
    onBackgroundTap,
  },
  ref
) {
  const containerRef = useRef<HTMLDivElement>(null);
  const cyRef = useRef<Core | null>(null);
  const simRef = useRef<Simulation<SimNode, SimLink> | null>(null);
  const nodesRef = useRef<Map<string, SimNode>>(new Map());
  const edgesRef = useRef<{ source: string; target: string }[]>([]);
  const tooltipRef = useRef<HTMLDivElement>(null);
  const [tooltip, setTooltip] = useState<TooltipState | null>(null);
  /**
   * Zoom/pan carried across a rebuild. React tears the old instance down in the
   * effect CLEANUP, which runs before the replacement effect body, so the new
   * build cannot read the outgoing viewport off `cyRef` — it has already been
   * destroyed and nulled. Stash it on the way out instead: an in-place reload
   * that re-framed the graph would throw away the view the user panned to.
   */
  const lastViewport = useRef<{ zoom: number; pan: { x: number; y: number } } | null>(null);

  // The effects below all read the CURRENT props, but the cytoscape event
  // handlers are registered once per payload and would close over the values
  // they were built with. Mirrored on refs so one build survives every later
  // change to the filter, the selection or a callback.
  const latest = useRef({ view, index, match, selectedIds, theme, gutter, measureGutter, forces });
  latest.current = { view, index, match, selectedIds, theme, gutter, measureGutter, forces };
  const handlers = useRef({ onPaperTap, onAuthorTap, onTagTap, onBackgroundTap });
  handlers.current = { onPaperTap, onAuthorTap, onTagTap, onBackgroundTap };

  /**
   * One-shot: reframe the next time the simulation settles. Armed by a cold load
   * and by "Randomize & restart" — both hand d3 a square of random seed
   * positions that the layout then spreads well past, so the viewport in force
   * at that moment frames something that no longer exists. Cleared on the first
   * grab so a reframe never yanks the viewport out from under a drag.
   */
  const fitOnSettle = useRef(false);
  /** A fit skipped because the viewport was 0x0 (the page is kept alive behind
   *  `display: none` while the user is elsewhere); replayed on the resize the
   *  reveal fires. */
  const fitDeferred = useRef(false);

  const hideTooltip = useCallback(() => setTooltip(null), []);

  const fit = useCallback(() => {
    const cy = cyRef.current;
    if (!cy || cy.nodes().length === 0) return;
    const w = cy.width();
    const h = cy.height();
    if (!w || !h) {
      fitDeferred.current = true;
      return;
    }
    fitDeferred.current = false;

    // Frame the nodes the user can SEE, not the extent of everything the payload
    // holds: with "Show highlighted only" on and three papers matching, framing
    // the whole library collapsed those three into a speck in the middle of it.
    // An ordinary 8% ghost IS drawn — faintly, deliberately — so it stays inside
    // the frame; only a hidden type or an isolated non-match drops out.
    const framed = drawnCollection(cy, latest.current.match);
    const bb = (framed ?? cy.elements()).boundingBox();
    const viewport = fitViewport(bb, w, h, latest.current.measureGutter(), {
      min: cy.minZoom(),
      max: cy.maxZoom(),
    });
    if (viewport) cy.viewport(viewport);
    else cy.fit(framed ?? undefined, FIT_PADDING);
  }, []);

  /** Charge and collision both have to know the CURRENT layout membership: an
   *  excluded node is PINNED, not removed from the simulation, so at full
   *  strength it goes on shoving the matching nodes around from behind its 8%
   *  ghost — and d3 splits a collision correction by the SQUARE of the radii, so
   *  a zero radius means a member paired with a ghost takes none of it. */
  const chargeForce = useCallback(() => {
    const { match: m, forces: f } = latest.current;
    const ids = layoutIds(m);
    return forceManyBody<SimNode>().strength((n) => (ids.has(n.id) ? -f.repel : 0));
  }, []);

  const collideForce = useCallback(() => {
    const ids = layoutIds(latest.current.match);
    return forceCollide<SimNode>().radius((n) => (ids.has(n.id) ? COLLIDE_RADIUS : 0));
  }, []);

  const applyStyles = useCallback(() => {
    const cy = cyRef.current;
    if (!cy) return;
    const { match: m, selectedIds: selected, theme: t, index: idx } = latest.current;
    const anySelected = selected.size > 0;

    // Authors and tags joined to a selected paper are highlighted with it.
    const selAuthors = new Set<string>();
    const selTags = new Set<string>();
    for (const pid of selected) {
      for (const nid of idx.neighboursByPaper.get(pid) ?? []) {
        if (idx.typeById.get(nid) === "author") selAuthors.add(nid);
        else if (idx.typeById.get(nid) === "tag") selTags.add(nid);
      }
    }

    const matchedFor = (type: GraphNodeType) =>
      type === "paper" ? m.papers : type === "author" ? m.authors : m.tags;
    const selectedFor = (type: GraphNodeType, id: string) =>
      type === "paper" ? selected.has(id) : type === "author" ? selAuthors.has(id) : selTags.has(id);

    cy.batch(() => {
      cy.nodes().forEach((n) => {
        const type = n.data("type") as GraphNodeType;
        const id = n.id();
        const opacity = m.hiddenTypes.has(type)
          ? 0
          : opacityFor(matchedFor(type).has(id), selectedFor(type, id), anySelected, m.isolate);
        const style: Record<string, unknown> = { opacity, events: eventsFor(opacity) };
        if (type === "paper") {
          // The selection is painted on EVERY selected paper, including one an
          // attribute filter has excluded: such a node is an 8% ghost, not a
          // hidden one, so a Ctrl-click puts it straight into the selection and
          // withholding the highlight made that click change nothing visible at
          // all. The filter still owns opacity; the selection owns colour.
          style["background-color"] = selected.has(id) ? highlightColor(t) : paperColor(t);
        }
        n.style(style);
      });

      cy.edges().forEach((e) => {
        const sid = e.source().id();
        const tid = e.target().id();
        const srcType = e.source().data("type") as GraphNodeType;
        const tgtType = e.target().data("type") as GraphNodeType;
        // An edge is only as visible as its endpoints: hiding either type takes
        // the edge with it, so no line dangles into empty canvas.
        if (m.hiddenTypes.has(srcType) || m.hiddenTypes.has(tgtType)) {
          e.style({ opacity: 0, events: "no" });
          return;
        }
        const visible = matchedFor(srcType).has(sid) && matchedFor(tgtType).has(tid);
        const sel = selectedFor(srcType, sid) || selectedFor(tgtType, tid);
        const opacity = opacityFor(visible, sel, anySelected, m.isolate);
        e.style({ opacity, events: eventsFor(opacity) });
      });
    });
  }, []);

  // ── Build: one cytoscape + one simulation per payload ──────────────────────
  useEffect(() => {
    let cancelled = false;
    const container = containerRef.current;
    if (!container) return;

    // Seed surviving nodes from the OUTGOING layout and hold zoom/pan, so a
    // refresh or an option toggle starts from the current view instead of
    // re-randomising. Empty on the first build, which is what makes that one a
    // cold load without needing to be told.
    const previous = new Map<string, { x: number; y: number }>();
    nodesRef.current.forEach((n, id) => previous.set(id, { x: n.x, y: n.y }));
    // Surviving positions are the whole test for "this replaces an earlier
    // payload": a cold load has none, so it seeds randomly, fits, and arms the
    // fit-on-settle, while a refresh or an option toggle keeps both the settled
    // layout and the viewport.
    const preserveView = previous.size > 0;
    const prevViewport = preserveView ? lastViewport.current : null;

    // Wait for the label face before the first paint: cytoscape caches label
    // measurements on the renderer keyed by text and font STYLE, so labels
    // measured in the fallback face keep those widths for the session.
    void whenLabelFontReady().then(() => {
      if (cancelled) return;

      simRef.current?.stop();
      cyRef.current?.destroy();
      simRef.current = null;
      cyRef.current = null;
      setTooltip(null);
      fitDeferred.current = false;
      fitOnSettle.current = !preserveView;

      const nodeIds = [
        ...view.papers.map((p) => p.id),
        ...view.authors.map((a) => a.id),
        ...view.tags.map((t) => t.id),
      ];
      edgesRef.current = view.edges.map((e) => ({ source: e.source, target: e.target }));
      const simNodes = seedPositions(
        nodeIds,
        edgesRef.current,
        previous,
        layoutRng()
      ) as SimNode[];
      nodesRef.current = new Map(simNodes.map((n) => [n.id, n]));

      const cy = cytoscape({
        container,
        elements: [
          ...view.papers.map((p) => ({
            group: "nodes" as const,
            data: { id: p.id, type: "paper", label: p.label, source_id: p.source_id },
            position: { x: nodesRef.current.get(p.id)!.x, y: nodesRef.current.get(p.id)!.y },
          })),
          ...view.authors.map((a) => ({
            group: "nodes" as const,
            data: { id: a.id, type: "author", label: a.label, author_id: a.author_id },
            position: { x: nodesRef.current.get(a.id)!.x, y: nodesRef.current.get(a.id)!.y },
          })),
          ...view.tags.map((t) => ({
            group: "nodes" as const,
            data: { id: t.id, type: "tag", label: t.label },
            position: { x: nodesRef.current.get(t.id)!.x, y: nodesRef.current.get(t.id)!.y },
          })),
          ...view.edges.map((e) => ({
            group: "edges" as const,
            data: { source: e.source, target: e.target },
          })),
        ],
        style: graphStylesheet(latest.current.theme),
        layout: { name: "preset" },
        minZoom: MIN_ZOOM,
        maxZoom: MAX_ZOOM,
        // Cache a texture and drop edges while panning/zooming to keep the
        // viewport smooth as the node count grows.
        textureOnViewport: true,
        hideEdgesOnViewport: true,
      });
      cyRef.current = cy;

      if (prevViewport) cy.viewport(prevViewport);
      else fit();

      // Cache each node's handle so the per-tick sync skips a lookup per node
      // per frame.
      for (const n of simNodes) n.cyNode = cy.getElementById(n.id);

      cy.on("grab", "node", (e) => {
        fitOnSettle.current = false; // the user took control — don't reframe under them
        setTooltip(null);
        const n = nodesRef.current.get(e.target.id());
        if (n) {
          n.fx = n.x;
          n.fy = n.y;
          // The pin in fx/fy is the DRAG's from here on, which is exactly what
          // `filterPinned` records. A filter-excluded ghost is fully grabbable,
          // and it arrives here still flagged as the FILTER's pin — so a filter
          // pass that re-admitted the node mid-drag took the "release the pin I
          // own" branch and handed it back to the simulation while the mouse was
          // still down: d3 then moved it under charge and link forces between
          // mousemove events, so it drifted away from the pointer and snapped
          // back on the next one. `free` re-establishes the filter's pin if the
          // node is still excluded when the user lets go.
          n.filterPinned = false;
        }
        simRef.current?.alphaTarget(0.3).restart();
      });
      cy.on("drag", "node", (e) => {
        const n = nodesRef.current.get(e.target.id());
        if (!n) return;
        const pos = e.target.position();
        n.fx = pos.x;
        n.fy = pos.y;
      });
      // Releasing a drag hands the node back to the layout — unless an active
      // filter had EXCLUDED it, in which case the pin under the user's fingers
      // was the filter's own. A ghost is fully grabbable (only opacity 0 leaves
      // hit-testing), so nulling fx/fy there released a pin nothing was going to
      // restore and the ghost — charge 0, collision radius 0 — slid off towards
      // the origin on its own. Re-pin at the drop point instead.
      cy.on("free", "node", (e) => {
        const n = nodesRef.current.get(e.target.id());
        if (n) {
          if (!layoutIds(latest.current.match).has(n.id)) {
            if (n.fx == null) {
              n.fx = n.x;
              n.fy = n.y;
            }
            n.filterPinned = true;
          } else {
            n.fx = null;
            n.fy = null;
            n.filterPinned = false;
          }
        }
        simRef.current?.alphaTarget(0);
      });

      cy.on("tap", 'node[type = "paper"]', (e) => {
        const additive = e.originalEvent.ctrlKey || e.originalEvent.metaKey;
        if (!additive) setTooltip(null);
        handlers.current.onPaperTap(e.target.id(), additive);
      });
      // Ctrl/Cmd is reserved for paper multi-select, so it is a no-op on the
      // other two types rather than a navigation.
      cy.on("tap", 'node[type = "author"]', (e) => {
        if (e.originalEvent.ctrlKey || e.originalEvent.metaKey) return;
        const authorId = e.target.data("author_id") as number | undefined;
        if (authorId === undefined || authorId === null) return;
        setTooltip(null);
        handlers.current.onAuthorTap(authorId);
      });
      cy.on("tap", 'node[type = "tag"]', (e) => {
        if (e.originalEvent.ctrlKey || e.originalEvent.metaKey) return;
        const label = e.target.data("label") as string | undefined;
        if (!label) return;
        setTooltip(null);
        handlers.current.onTagTap(label);
      });
      cy.on("tap", (e) => {
        if (e.target !== cy) return;
        if (e.originalEvent.ctrlKey || e.originalEvent.metaKey) return;
        handlers.current.onBackgroundTap();
      });

      cy.on("mouseover", "node", (e) => showTooltipFor(e.target));
      cy.on("mouseout", "node", () => setTooltip(null));
      // The box is placed in rendered (screen) coordinates, so a pan or zoom
      // would leave it pointing at empty canvas.
      cy.on("viewport", () => setTooltip(null));

      const simLinks: SimLink[] = edgesRef.current.map((e) => ({ ...e }));
      const sim = forceSimulation<SimNode>(simNodes)
        .force(
          "link",
          forceLink<SimNode, SimLink>(simLinks)
            .id((d) => d.id)
            .distance(latest.current.forces.linkDistance)
            .strength(latest.current.forces.linkStrength)
        )
        .force("charge", chargeForce())
        .force("x", forceX<SimNode>(0).strength(latest.current.forces.center))
        .force("y", forceY<SimNode>(0).strength(latest.current.forces.center))
        .force("collision", collideForce());
      simRef.current = sim;

      sim.on("tick", () => {
        cy.batch(() => {
          for (const n of simNodes) n.cyNode?.position({ x: n.x, y: n.y });
        });
      });
      // Frame the settled layout rather than the random seed positions fitted
      // above. Fires again after every drag/filter restart, so the one-shot flag
      // is what keeps it from yanking the viewport out from under the user.
      sim.on("end", () => {
        if (!fitOnSettle.current) return;
        fitOnSettle.current = false;
        fit();
      });

      lastLayoutIds.current = null;
      // A cold load is starting from random seeds and must expand out of them;
      // an in-place reload is starting from the settled layout and must not.
      applyPhysics(true, preserveView ? 0.3 : 1);
      applyStyles();

      function showTooltipFor(node: NodeSingular) {
        const { index: idx, match: m } = latest.current;
        const type = node.data("type") as GraphNodeType;
        const content = tooltipFor(node.id(), type, idx, drawnPapers(m));
        const rendered = node.renderedPosition();
        // Placed from the node's rendered position with a nominal box size; the
        // layout effect below re-measures once it is in the DOM and flips it if
        // it would overhang. Measuring first would need a hidden render pass.
        setTooltip({ ...content, left: rendered.x + 14, top: rendered.y + 14 });
      }
    });

    return () => {
      cancelled = true;
      if (cyRef.current) {
        lastViewport.current = { zoom: cyRef.current.zoom(), pan: { ...cyRef.current.pan() } };
      }
      simRef.current?.stop();
      cyRef.current?.destroy();
      simRef.current = null;
      cyRef.current = null;
    };
    // Rebuilt only when the PAYLOAD changes. Theme, forces, filter and selection
    // all ride the effects below, which drive the live instance in place.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view]);

  /**
   * Pin the excluded nodes, restrict the link force to edges inside the layout,
   * rebuild charge/collision for the new membership, and reheat the simulation.
   *
   * Skipped entirely when the membership is UNCHANGED, because reheating is not
   * free: it pushes every unpinned node around for a few hundred more ticks.
   * "Show highlighted only" produces a brand-new `match` object while leaving
   * `layoutIds` byte-identical — it changes what is DRAWN, not what the layout
   * runs over — so keying off the object alone made a pure view toggle visibly
   * shuffle the papers the user had arranged. A Visibility checkbox is NOT such
   * a toggle any more: it takes its whole type out of `layoutIds`, so it lands
   * here as a genuine membership change and reheats on purpose — an invisible
   * node must not go on shaping the layout of the visible ones.
   *
   * `force` is for the callers that need the work done regardless: a fresh
   * payload (nothing has been applied to this simulation yet) and "Randomize &
   * restart", which clears every pin — the filter's included — and so must have
   * them re-established even though no node changed membership.
   *
   * `alpha` is how hard to anneal afterwards, and the callers genuinely differ.
   * 0.3 is a nudge: the layout is already settled and only the membership moved,
   * so the nodes should barely shift. A layout starting from RANDOM seeds needs
   * the full 1 — d3 applies every force in proportion to alpha and decays it
   * geometrically, so the total impulse is `alpha / alphaDecay` and 0.3 delivers
   * about a third of it. That is not enough to expand out of the 800x800 seed
   * box, which is the whole reason a cold load re-fits once it settles.
   */
  const lastLayoutIds = useRef<Set<string> | null>(null);
  const applyPhysics = useCallback((force = false, alpha = 0.3) => {
    const sim = simRef.current;
    if (!sim) return;
    const ids = layoutIds(latest.current.match);
    if (!force && lastLayoutIds.current && sameIds(lastLayoutIds.current, ids)) return;
    lastLayoutIds.current = ids;

    nodesRef.current.forEach((n) => {
      if (!ids.has(n.id)) {
        // `fx == null` means nothing holds this node yet. A node already pinned
        // here is being dragged (see `grab`), and that pin outranks the
        // filter's — overwriting it would fight the cursor.
        if (n.fx == null) {
          n.fx = n.x;
          n.fy = n.y;
          n.filterPinned = true;
        }
      } else if (n.filterPinned) {
        // Release only a pin the FILTER owns. `grab` clears this flag for the
        // duration of a drag, so a node re-admitted mid-drag stays under the
        // cursor; `free` decides which owner the pin belongs to at the drop.
        n.fx = null;
        n.fy = null;
        n.filterPinned = false;
      }
    });

    const active = edgesRef.current
      .filter((e) => ids.has(e.source) && ids.has(e.target))
      .map((e) => ({ ...e }));
    (sim.force("link") as ForceLink<SimNode, SimLink>).links(active);
    sim.force("charge", chargeForce());
    sim.force("collision", collideForce());
    sim.alpha(alpha).restart();
  }, [chargeForce, collideForce]);

  useEffect(() => {
    // Styles always; physics only if the layout membership actually moved.
    applyPhysics();
    applyStyles();
  }, [match, applyPhysics, applyStyles]);

  useEffect(() => {
    applyStyles();
  }, [selectedIds, applyStyles]);

  // A theme change reinstalls the stylesheet AND repaints: every paper node
  // carries a per-element `background-color` bypass (it is how selection state
  // is painted) and a bypass outranks the stylesheet, so without the repaint the
  // papers stay on the OLD accent while tags, edges and labels follow the new one.
  useEffect(() => {
    const cy = cyRef.current;
    if (!cy) return;
    cy.style(graphStylesheet(theme)).update();
    applyStyles();
  }, [theme, applyStyles]);

  useEffect(() => {
    const sim = simRef.current;
    if (!sim) return;
    const link = sim.force("link") as ForceLink<SimNode, SimLink>;
    link.distance(forces.linkDistance).strength(forces.linkStrength);
    sim.force("charge", chargeForce());
    sim.force("x", forceX<SimNode>(0).strength(forces.center));
    sim.force("y", forceY<SimNode>(0).strength(forces.center));
    sim.alpha(0.3).restart();
  }, [forces, chargeForce]);

  // Revealing the page takes the container from 0x0 to its real size; run the
  // fit that was skipped while there was nothing to fit.
  useEffect(() => {
    const container = containerRef.current;
    if (!container || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => {
      cyRef.current?.resize();
      if (fitDeferred.current) fit();
    });
    ro.observe(container);
    return () => ro.disconnect();
  }, [fit]);

  // Keep the inspector inside the canvas and clear of the panel column. Run
  // after paint, when the box has a real size to flip against.
  useEffect(() => {
    if (!tooltip) return;
    const box = tooltipRef.current;
    const container = containerRef.current;
    if (!box || !container) return;
    const rect = box.getBoundingClientRect();
    const bounds = container.getBoundingClientRect();
    const placed = placeFloatingBox(
      { x: tooltip.left - 14, y: tooltip.top - 14 },
      { width: rect.width, height: rect.height },
      { width: bounds.width, height: bounds.height, gutter }
    );
    if (placed.left !== tooltip.left || placed.top !== tooltip.top) {
      setTooltip((t) => (t ? { ...t, left: placed.left, top: placed.top } : t));
    }
    // Only re-place when a NEW box appears; re-running on every position write
    // would loop against its own setState.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tooltip?.title, tooltip?.meta, tooltip?.summary, gutter]);

  useImperativeHandle(
    ref,
    () => ({
      relayout() {
        const sim = simRef.current;
        if (!sim) return;
        // The inspector is placed against a node that is about to jump.
        setTooltip(null);
        const nodes = [...nodesRef.current.values()];
        randomizePositions(nodes, layoutRng());
        for (const n of nodes) {
          n.vx = 0;
          n.vy = 0;
          n.fx = null;
          n.fy = null;
          n.filterPinned = false;
        }
        // Randomize clears the FILTER's pins along with the drag pins, so the
        // excluded nodes have to be re-pinned at their new seed positions or
        // they drift under the centring force as visible 8% ghosts.
        fitOnSettle.current = true;
        // alpha 1, the same as a cold load: this re-seeds into the very same
        // 800x800 box, and the layout has to expand out of it again.
        applyPhysics(true, 1);
      },
    }),
    [applyPhysics]
  );

  return (
    <>
      {/* Inline, not `absolute inset-0`: cytoscape injects an unlayered
          `position: relative` on its container class, which outranks
          Tailwind 4's layered utilities and collapses the div to 0 height. */}
      <div
        ref={containerRef}
        style={{ position: "absolute", inset: 0 }}
        onMouseLeave={hideTooltip}
      />
      {tooltip && (
        <div
          ref={tooltipRef}
          role="tooltip"
          className="absolute z-20 pointer-events-none max-w-[340px] rounded-md border border-border px-3 py-2 shadow-lg"
          style={{ left: tooltip.left, top: tooltip.top, backgroundColor: "var(--color-panel)" }}
        >
          <div className="text-sm font-semibold text-text">
            <MathText forceInline>{tooltip.title}</MathText>
          </div>
          {tooltip.meta.length > 0 && (
            <div className="mt-1 text-xs text-muted whitespace-pre-line">
              {tooltip.meta.join("\n")}
            </div>
          )}
          {tooltip.summary && (
            <div className="mt-1 text-xs text-muted">
              <MathText forceInline>{tooltip.summary}</MathText>
            </div>
          )}
        </div>
      )}
    </>
  );
});

export default GraphCanvas;

/** Whether two layout-membership sets name the same nodes. */
function sameIds(a: ReadonlySet<string>, b: ReadonlySet<string>): boolean {
  if (a.size !== b.size) return false;
  for (const id of a) if (!b.has(id)) return false;
  return true;
}

/** The papers currently painted at a non-zero opacity — what a degree line
 *  reports as "shown". */
function drawnPapers(m: GraphMatch): Set<string> {
  return m.hiddenTypes.has("paper") ? new Set() : new Set(m.papers);
}

/** The nodes a fit should frame: the ones left at a non-zero opacity. `null`
 *  means "nothing is being held back, frame the whole graph". */
function drawnCollection(cy: Core, m: GraphMatch) {
  const isolating = m.isolate;
  if (m.hiddenTypes.size === 0 && !isolating) return null;
  const matchedFor = (type: GraphNodeType) =>
    type === "paper" ? m.papers : type === "author" ? m.authors : m.tags;
  const drawn = cy.nodes().filter((n) => {
    const type = n.data("type") as GraphNodeType;
    if (m.hiddenTypes.has(type)) return false;
    if (!isolating) return true;
    return matchedFor(type).has(n.id());
  });
  // Isolate with a filter matching nothing draws an empty canvas; there is no
  // extent to frame, so fall back to the whole graph rather than a degenerate box.
  return drawn.length > 0 ? drawn : null;
}
