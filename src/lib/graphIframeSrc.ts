import type { ThemeColors, ThemeMode } from "./theme";

/** Path of the Knowledge Graph iframe document, served straight out of `public/`. */
export const GRAPH_IFRAME_PATH = "/graph/graph.html";

/**
 * Which backend the guest should fetch its data from.
 *
 * `"linxiv"` is the in-process library over the custom scheme (the transport
 * `src/api/client.ts` reaches through `invoke`, and `papers.ts` builds URLs for
 * with `linxivUrl`); `"origin"` is the iframe's own origin, i.e. browser dev,
 * where Vite proxies `/api` to a separate dev server.
 *
 * The guest cannot work this out for itself: `tauri dev` and browser dev BOTH
 * serve it from `http://localhost:5180`, and only the host knows which one it
 * is (`isTauri`). Sniffing the URL sent the graph — alone in the app — to the
 * dev server's database under `tauri dev`. The mapping lives with the caller,
 * next to the rest of the app's `isTauri` branches; graph.js's own list of
 * accepted values is pinned equal to this one in graphIframeAssets.test.ts.
 */
export type GraphApiTransport = "linxiv" | "origin";

/**
 * Build the Knowledge Graph iframe `src`.
 *
 * All four params are read exactly once, before paint:
 *  - `excludeSingleAuthors` by graph.js's bootstrap (it picks the first
 *    `/api/graph` query; later toggles ride the `set_options` postMessage);
 *  - `api` by the same bootstrap, naming the backend transport it fetches over
 *    (see `GraphApiTransport`);
 *  - `mode` by the same pre-paint script, which sets `color-scheme` from it.
 *    That is the light/dark half of the theme, which the eight `ThemeColors`
 *    tokens do not encode; `src/styles/tokens.css` sets it from `[data-mode]`
 *    for the host, and the iframe inherits nothing, so without it every native
 *    control in the graph's panels (date pickers, checkboxes, the force
 *    sliders, the panel scrollbar) sat on the UA default of light inside a
 *    dark preset. Later mode changes ride `theme_update`;
 *  - `theme` by graph.html's inline pre-paint script, which sets one
 *    `--color-*` var per `ThemeColors` key for graph.css and graph.js to read
 *    (the lists are pinned equal in graphIframeTheme.test.ts). Passing the resolved
 *    `getColors()` result keeps `src/lib/theme.ts` the single source of truth —
 *    graph.html used to carry its own copy of the palette table, which went
 *    stale silently whenever a preset was added (the "Reading Room" preset fell
 *    back to Navy). Later theme changes ride the `theme_update` postMessage.
 *
 * The caller MUST freeze both values when the frame mounts: the src is an iframe
 * navigation, so changing it reloads the guest and drops its settled layout and
 * selection. GraphPage freezes them on the first visit to /graph rather than at
 * its own mount — AppShell's keep-alive mounts that page at app boot, hours of
 * theme changes before the frame is ever shown.
 */
export function graphIframeSrc(opts: {
  excludeSingleAuthors: boolean;
  api: GraphApiTransport;
  mode: ThemeMode;
  theme: ThemeColors;
}): string {
  const params = new URLSearchParams();
  if (opts.excludeSingleAuthors) params.set("excludeSingleAuthors", "1");
  params.set("api", opts.api);
  params.set("mode", opts.mode);
  params.set("theme", JSON.stringify(opts.theme));
  return `${GRAPH_IFRAME_PATH}?${params}`;
}
