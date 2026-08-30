// Seed positions and force parameters for the graph's d3-force layout.
//
// Pure functions over plain `{id, x, y}` records, so the rules that decide where
// a node starts — the one part of the layout that is not d3's business — are
// testable without a canvas.

/** The force-simulation knobs the Layout panel drives. */
export interface ForceSettings {
  center: number;
  repel: number;
  linkDistance: number;
  linkStrength: number;
}

export const DEFAULT_FORCES: ForceSettings = {
  center: 0.05,
  repel: 180,
  linkDistance: 70,
  linkStrength: 0.3,
};

/** The square of random start positions a cold load hands d3, centred on the
 *  world origin the centring force pulls towards. */
export const SEED_SPREAD = 800;
/** Spread around a neighbour centroid. Nodes seeded at the exact same point give
 *  the repulsion no direction to push them apart, and papers imported together
 *  share their whole neighbourhood. */
export const SEED_JITTER = 40;
/**
 * Rounds of "place every node that now has a placed neighbour". Two is what an
 * imported paper needs: the first puts the PAPER beside the authors and tags it
 * shares with the existing library, the second puts its brand-new author nodes
 * beside the paper. Anything still unreached is an entirely new island and keeps
 * its box seed, which is the honest answer — there is nothing to sit by.
 */
export const SEED_PASSES = 2;

/**
 * Reproducible layout (opt-in). Initial positions are normally `Math.random()`,
 * which is fine for real use but makes a scripted demo recording a lottery. If
 * this localStorage key holds a value, a small deterministic PRNG is seeded from
 * it instead; absent — the default for every real user — nothing changes.
 */
export const LAYOUT_SEED_KEY = "linxiv-graph-seed";

export function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** Re-seeded fresh on each load and relayout, so "same seed" means "same
 *  sequence" regardless of how many times the page has reloaded. */
export function layoutRng(storage?: Pick<Storage, "getItem">): () => number {
  const store = storage ?? (typeof localStorage === "undefined" ? null : localStorage);
  const seed = store?.getItem(LAYOUT_SEED_KEY);
  if (!seed) return Math.random;
  return mulberry32(parseInt(seed, 10) || 0);
}

export interface SimNode {
  id: string;
  x: number;
  y: number;
}

/**
 * Start positions for one load.
 *
 * A node that survived the previous layout keeps its settled position. A node
 * that is NEW gets seeded at the centroid of the neighbours that do have one,
 * because the cold-load answer — a random point in an 800x800 box at the origin
 * — is the wrong one for it twice over: the force layout normally spreads the
 * graph far wider than that box, so a paper imported elsewhere in the app would
 * appear nowhere near the authors and tags it is joined to and then travel the
 * whole canvas under the link force; and every new node arriving in the same
 * clump at the centre shoves that settled neighbourhood apart, under a viewport
 * an in-place reload deliberately does not reframe.
 */
export function seedPositions(
  ids: readonly string[],
  edges: readonly { source: string; target: string }[],
  previous: ReadonlyMap<string, { x: number; y: number }>,
  rand: () => number
): SimNode[] {
  const placed = new Set<string>();
  const nodes: SimNode[] = ids.map((id) => {
    const prev = previous.get(id);
    if (prev) {
      placed.add(id);
      return { id, x: prev.x, y: prev.y };
    }
    return { id, x: (rand() - 0.5) * SEED_SPREAD, y: (rand() - 0.5) * SEED_SPREAD };
  });
  seedNewFromNeighbours(nodes, edges, placed, rand);
  return nodes;
}

function seedNewFromNeighbours(
  nodes: SimNode[],
  edges: readonly { source: string; target: string }[],
  placed: Set<string>,
  rand: () => number
): void {
  if (placed.size === 0 || placed.size === nodes.length) return;
  const byId = new Map(nodes.map((n) => [n.id, n]));
  // Undirected adjacency over THIS payload's edges: a new paper reaches its
  // neighbours through the source side and a new author through the target side.
  const adjacency = new Map<string, string[]>();
  const link = (a: string, b: string) => {
    const list = adjacency.get(a);
    if (list) list.push(b);
    else adjacency.set(a, [b]);
  };
  for (const e of edges) {
    if (!byId.has(e.source) || !byId.has(e.target)) continue;
    link(e.source, e.target);
    link(e.target, e.source);
  }

  for (let pass = 0; pass < SEED_PASSES; pass++) {
    // Collected and applied per pass rather than as we go, so a node placed in
    // this pass cannot seed another one in the same pass — the result would
    // then depend on the order the payload happened to list nodes in.
    const seeded: string[] = [];
    for (const n of nodes) {
      if (placed.has(n.id)) continue;
      let sx = 0;
      let sy = 0;
      let count = 0;
      for (const id of adjacency.get(n.id) ?? []) {
        if (!placed.has(id)) continue;
        const nb = byId.get(id)!;
        sx += nb.x;
        sy += nb.y;
        count++;
      }
      if (count === 0) continue;
      n.x = sx / count + (rand() - 0.5) * SEED_JITTER;
      n.y = sy / count + (rand() - 0.5) * SEED_JITTER;
      seeded.push(n.id);
    }
    if (seeded.length === 0) return;
    for (const id of seeded) placed.add(id);
  }
}

/** Fresh random seeds for "Randomize & restart" — the state a cold load is in. */
export function randomizePositions(nodes: SimNode[], rand: () => number): void {
  for (const n of nodes) {
    n.x = (rand() - 0.5) * SEED_SPREAD;
    n.y = (rand() - 0.5) * SEED_SPREAD;
  }
}
