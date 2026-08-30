// A small `GET /api/graph` payload for the graph tests, shaped exactly as
// `linxiv_core::graph::graph_view` emits it: paper node ids are the SOURCE_FK
// stringified, author nodes are `author::<AUTHOR_FK>`, tag nodes `tag::<key>`,
// and every derivation the backend owns (canonical tag spellings, the
// `0001-01-01` fold, the lowercased author index, the degrees) is already done.

import type { GraphPaper, GraphProject, GraphView } from "./model.ts";

export function paper(over: Partial<GraphPaper> & { id: string }): GraphPaper {
  return {
    source_fk: Number(over.id),
    source_id: `arxiv:${over.id}`,
    label: "Untitled",
    category: null,
    tags: [],
    tag_keys: [],
    has_pdf: false,
    published: null,
    url: null,
    doi: null,
    summary: null,
    project_ids: [],
    author_keys: [],
    ...over,
  };
}

export function project(over: Partial<GraphProject> & { id: number }): GraphProject {
  return {
    name: `Project ${over.id}`,
    color: "#5b8dee",
    tags: [],
    on_graph: true,
    ...over,
  };
}

/**
 * Two papers, one shared author, two tags.
 *
 *   1 "Attention Is All You Need"  cs.LG  2024-01-01  PDF   tags ML, nlp
 *   2 "Other Work"                 cs.CL  undated     no    tags nlp
 *   author::7 Ada Lovelace  — on both
 *   author::8 Alan Turing   — on paper 1 only
 */
export function sampleView(): GraphView {
  return {
    papers: [
      paper({
        id: "1",
        label: "Attention Is All You Need",
        category: "cs.LG",
        tags: ["ML", "nlp"],
        tag_keys: ["ml", "nlp"],
        has_pdf: true,
        published: "2024-01-01",
        summary: "We propose a new architecture based purely on attention.",
        project_ids: [10],
        author_keys: ["ada lovelace", "alan turing"],
      }),
      paper({
        id: "2",
        label: "Other Work",
        category: "cs.CL",
        tags: ["nlp"],
        tag_keys: ["nlp"],
        published: null,
        author_keys: ["ada lovelace"],
      }),
    ],
    authors: [
      { id: "author::7", author_id: 7, label: "Ada Lovelace", paper_count: 2 },
      { id: "author::8", author_id: 8, label: "Alan Turing", paper_count: 1 },
    ],
    tags: [
      { id: "tag::ml", key: "ml", label: "ML", paper_count: 1 },
      { id: "tag::nlp", key: "nlp", label: "nlp", paper_count: 2 },
    ],
    edges: [
      { source: "1", target: "author::7" },
      { source: "1", target: "author::8" },
      { source: "1", target: "tag::ml" },
      { source: "1", target: "tag::nlp" },
      { source: "2", target: "author::7" },
      { source: "2", target: "tag::nlp" },
    ],
    categories: ["cs.CL", "cs.LG"],
    projects: [project({ id: 10, name: "Transformers", tags: ["reading"] })],
  };
}
