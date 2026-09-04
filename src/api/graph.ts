import { libraryFetch } from "../stores/backend.ts";
import type { GraphView } from "../types/generated";

export type { GraphView };

/**
 * `GET /api/graph` — the whole Knowledge Graph payload.
 *
 * One request, over the app's ordinary transport. The graph used to be an
 * iframe that fetched for itself over the `linxiv://` custom scheme, which meant
 * it needed FOUR requests (graph, project options, categories, tags), a
 * partial-failure protocol so a dead dropdown endpoint could not fail a load the
 * graph request had succeeded at, and a `?api=` parameter naming which backend
 * to talk to — because `tauri dev` and browser dev both serve the guest from
 * http://localhost:5180 and only the host could tell them apart. Going through
 * `libraryFetch` makes all three questions somebody else's, already-answered ones.
 *
 * @param excludeSingleAuthors drop authors linked to only one paper. Applied by
 * the backend, so those authors leave the payload entirely — the page's Author
 * filter matches `GraphPaper.author_keys`, which is narrowed with them.
 */
export async function getGraphView(excludeSingleAuthors: boolean): Promise<GraphView> {
  const q = excludeSingleAuthors ? "?exclude_single_authors=true" : "";
  return libraryFetch<GraphView>(`/api/graph${q}`);
}
