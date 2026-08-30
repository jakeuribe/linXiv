// Shared vocabulary for the Knowledge Graph page.
//
// The wire types are GENERATED from the Rust structs in
// `src-tauri/crates/core/src/graph.rs` (see src/types/generated.ts) — the graph
// is the one surface in the app whose payload used to be assembled by an inline
// `json!` and consumed by an unbundled browser script, so nothing type-checked
// the join between them. Everything here is derived from those types.

import type {
  GraphAuthor,
  GraphEdge,
  GraphPaper,
  GraphProject,
  GraphTag,
  GraphView,
} from "../../types/generated.ts";

export type {
  GraphAuthor,
  GraphEdge,
  GraphPaper,
  GraphProject,
  GraphTag,
  GraphView,
};

export type GraphNodeType = "paper" | "author" | "tag";

/** A node as cytoscape holds it: one id space over all three types. */
export interface GraphNodeData {
  id: string;
  type: GraphNodeType;
  label: string;
  /** Papers only — the vocabulary the project picker speaks. */
  source_id?: string;
  /** Authors only — the `/authors/:id` route param. */
  author_id?: number;
  /** Authors and tags only — papers on this canvas joined to the node. */
  paper_count?: number;
}

/**
 * The lookups the filter and the canvas both need, built once per payload.
 *
 * The old iframe rebuilt every one of these on each load by walking the edge
 * list in JavaScript; the parts that are facts about the LIBRARY (a paper's
 * author names, a tag's canonical spelling, a node's degree) now ride on the
 * payload, so this is only the id-space bookkeeping that is genuinely local.
 */
export interface GraphIndex {
  paperById: Map<string, GraphPaper>;
  authorById: Map<string, GraphAuthor>;
  tagById: Map<string, GraphTag>;
  projectById: Map<number, GraphProject>;
  /** Node id -> its type, for edge endpoints and style passes. */
  typeById: Map<string, GraphNodeType>;
  /** Paper node id -> the author/tag node ids it is joined to. */
  neighboursByPaper: Map<string, string[]>;
  /** Author or tag node id -> the paper node ids joined to it. */
  papersByNode: Map<string, string[]>;
}

export function indexView(view: GraphView): GraphIndex {
  const paperById = new Map(view.papers.map((p) => [p.id, p]));
  const authorById = new Map(view.authors.map((a) => [a.id, a]));
  const tagById = new Map(view.tags.map((t) => [t.id, t]));
  const projectById = new Map(view.projects.map((p) => [p.id, p]));

  const typeById = new Map<string, GraphNodeType>();
  for (const p of view.papers) typeById.set(p.id, "paper");
  for (const a of view.authors) typeById.set(a.id, "author");
  for (const t of view.tags) typeById.set(t.id, "tag");

  const neighboursByPaper = new Map<string, string[]>();
  const papersByNode = new Map<string, string[]>();
  for (const e of view.edges) {
    push(neighboursByPaper, e.source, e.target);
    push(papersByNode, e.target, e.source);
  }
  return {
    paperById,
    authorById,
    tagById,
    projectById,
    typeById,
    neighboursByPaper,
    papersByNode,
  };
}

function push<K, V>(map: Map<K, V[]>, key: K, value: V): void {
  const list = map.get(key);
  if (list) list.push(value);
  else map.set(key, [value]);
}

/**
 * The one normalization every tag comparison on this page goes through, and the
 * exact rule `linxiv_core::graph::norm_tag` applies server-side: trim, then fold
 * to lower case.
 *
 * A tag row is free text the user typed, so it has to be folded here; the values
 * it is compared against (`GraphPaper.tag_keys`, `GraphTag.key`) arrive already
 * normalized, which is what stops the two sides disagreeing over "ML" / "ml " /
 * "ml" the way they used to.
 */
export function normTag(raw: string): string {
  return raw.trim().toLowerCase();
}
