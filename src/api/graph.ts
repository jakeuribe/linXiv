import { libraryFetch } from "../stores/backend.ts";
import type { GraphView } from "../types/generated";

export type { GraphView };

/**
 * `GET /api/graph` — the whole Knowledge Graph payload (nodes, edges, project
 * options, categories, tags) in one request over `libraryFetch`.
 *
 * @param excludeSingleAuthors drop authors linked to only one paper. Applied by
 * the backend, so those authors leave the payload entirely — the page's Author
 * filter matches `GraphPaper.author_keys`, which is narrowed with them.
 */
export async function getGraphView(excludeSingleAuthors: boolean): Promise<GraphView> {
  const q = excludeSingleAuthors ? "?exclude_single_authors=true" : "";
  return libraryFetch<GraphView>(`/api/graph${q}`);
}
