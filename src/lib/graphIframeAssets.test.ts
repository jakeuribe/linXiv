// Run: node --experimental-transform-types --test src/lib/graphIframeAssets.test.ts
//
// public/graph/ is copied verbatim into the bundle by Vite and served over
// linxiv:// — nothing imports it, so the bundler can neither tree-shake it nor
// warn about a <script src> pointing at a file that is not there. It had
// accumulated 233 KB of vendored layout libraries (cytoscape-fcose,
// avsdf-base, layout-base) that graph.html never loaded: the graph runs a D3
// force simulation and only uses cytoscape as the renderer. These two checks
// are the drift guard in both directions.
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";

const GRAPH_DIR = path.join(import.meta.dirname, "../../public/graph");
const html = fs.readFileSync(path.join(GRAPH_DIR, "graph.html"), "utf8");

/** Every relative `<script src>` / `<link href>` graph.html loads. */
function referencedAssets(): string[] {
  const refs = [...html.matchAll(/<(?:script|link)\b[^>]*\b(?:src|href)="([^"]+)"/g)]
    .map((m) => m[1])
    .filter((ref) => !/^[a-z]+:|^\/\//i.test(ref));
  assert.ok(refs.length > 0, "graph.html must load its assets by relative path");
  return refs;
}

test("every asset graph.html references exists on disk", () => {
  for (const ref of referencedAssets()) {
    assert.ok(
      fs.existsSync(path.join(GRAPH_DIR, ref)),
      `graph.html loads ${ref}, which is missing from public/graph/`
    );
  }
});

test("public/graph ships nothing graph.html does not load", () => {
  const loaded = new Set([...referencedAssets(), "graph.html"]);
  const shipped = fs.readdirSync(GRAPH_DIR);
  const dead = shipped.filter((f) => !loaded.has(f));
  assert.deepEqual(
    dead,
    [],
    `public/graph is copied whole into the bundle, so these are dead weight: ${dead.join(", ")}`
  );
});

// ── Typeface ────────────────────────────────────────────────────────────────
// The iframe is a separate document, so it inherits nothing from
// src/styles/globals.css. graph.css used to ask for 'Segoe UI', a Windows-only
// face, so on Linux and macOS the graph rendered in a generic sans inside an
// app rendering Inter. It now re-declares the host's own self-hosted faces —
// these guards keep that declaration from drifting back apart.

const graphCss = fs.readFileSync(path.join(GRAPH_DIR, "graph.css"), "utf8");
const graphJs = fs.readFileSync(path.join(GRAPH_DIR, "graph.js"), "utf8");
const globalsCss = fs.readFileSync(
  path.join(import.meta.dirname, "../styles/globals.css"),
  "utf8"
);

/** A CSS font stack, comparable across quoting/spacing styles. */
function normalizeStack(stack: string): string {
  return stack
    .split(",")
    .map((f) => f.trim().replace(/^["']|["']$/g, "").toLowerCase())
    .join(", ");
}

/** The `font-family` of the first rule whose selector list matches `selector`. */
function bodyFontStack(css: string, selector: RegExp): string {
  const block = css.match(new RegExp(`${selector.source}\\s*\\{([^}]*)\\}`));
  assert.ok(block, `no rule matching ${selector} to read a font stack from`);
  const decl = block![1].match(/font-family:\s*([^;]+);/);
  assert.ok(decl, `rule matching ${selector} declares no font-family`);
  return normalizeStack(decl![1]);
}

/** Every `url(...)` in a stylesheet, unquoted. */
function cssUrls(css: string): string[] {
  return [...css.matchAll(/url\(\s*["']?([^"')]+)["']?\s*\)/g)].map((m) => m[1]);
}

test("the graph iframe renders in the same typeface as the app", () => {
  const host = bodyFontStack(globalsCss, /html,\s*body,\s*#root/);
  assert.equal(
    bodyFontStack(graphCss, /body/),
    host,
    "graph.css's body stack must match globals.css's — the iframe inherits nothing"
  );

  // Cytoscape draws node labels on a canvas and takes a bare CSS stack.
  const label = graphJs.match(/const LABEL_FONT\s*=\s*(.+);/);
  assert.ok(label, "graph.js must build its cytoscape label stack from LABEL_FONT");
  const family = graphJs.match(/const LABEL_FONT_FAMILY\s*=\s*'([^']+)'/);
  assert.ok(family, "graph.js must name the label font family in LABEL_FONT_FAMILY");
  const resolved = label![1].replace(/LABEL_FONT_FAMILY/, `'${family![1]}'`);
  assert.equal(
    normalizeStack(resolved.replace(/['\s]|\+/g, " ")),
    host,
    "graph.js's cytoscape label stack must match globals.css's"
  );

  // Nothing may reintroduce a per-rule override alongside the body stack.
  const stacks = [...graphCss.matchAll(/font-family:\s*([^;]+);/g)]
    .map((m) => normalizeStack(m[1]))
    .filter((s) => s !== "inherit");
  const strays = stacks.filter((s) => s !== host && s !== "inter");
  assert.deepEqual(strays, [], "graph.css declares a font stack the app does not use");
});

test("the graph iframe loads the app's own font files", () => {
  const faces = (css: string) =>
    new Set(
      cssUrls(css)
        .filter((u) => /\.woff2?$/.test(u))
        .map((u) => path.basename(u))
    );
  const hostFaces = faces(globalsCss);
  assert.ok(hostFaces.size > 0, "globals.css must self-host the app font");
  assert.deepEqual(
    [...faces(graphCss)].sort(),
    [...hostFaces].sort(),
    "graph.css must @font-face exactly the faces globals.css ships — a family it " +
      "never loads falls back silently"
  );

  // graph.css is copied verbatim by Vite, so a wrong relative path is a 404 at
  // runtime with no build error. Resolve each against the file's own location.
  for (const url of cssUrls(graphCss)) {
    assert.ok(
      fs.existsSync(path.resolve(GRAPH_DIR, url)),
      `graph.css requests ${url}, which does not resolve inside public/`
    );
  }
});

// ── postMessage protocol ────────────────────────────────────────────────────
// The iframe boundary is the one place in this app where the two sides of a
// call are not type-checked against each other: graph.js is outside the
// bundler, so a message one side starts sending and the other never learned to
// handle is silently a no-op — exactly how a tag node ended up being the only
// node type that did nothing when clicked. Pin the two message vocabularies
// equal in both directions.

const graphPage = fs.readFileSync(
  path.join(import.meta.dirname, "../pages/GraphPage.tsx"),
  "utf8"
);

/** Distinct capture-group-1 matches, sorted. */
function names(src: string, re: RegExp): string[] {
  return [...new Set([...src.matchAll(re)].map((m) => m[1]))].sort();
}

test("every message the graph iframe posts is handled by GraphPage", () => {
  const posted = names(graphJs, /\{\s*type:\s*'([a-z_]+)'/g);
  assert.ok(posted.length > 0, "graph.js must post something to its host");
  assert.deepEqual(
    posted,
    names(graphPage, /e\.data\.type === "([a-z_]+)"/g),
    "graph.js and GraphPage disagree on the guest→host message vocabulary"
  );
});

test("every message GraphPage posts is handled by the graph iframe", () => {
  const posted = names(graphPage, /\{\s*type:\s*"([a-z_]+)"/g);
  assert.ok(posted.length > 0, "GraphPage must post something to its guest");
  assert.deepEqual(
    posted,
    names(graphJs, /e\.data\.type === '([a-z_]+)'/g),
    "GraphPage and graph.js disagree on the host→guest message vocabulary"
  );
});

// ── Form controls ───────────────────────────────────────────────────────────

test("the graph enters dates the way the rest of the app does", () => {
  // The Date range boxes were free text with a "YYYY-MM-DD" placeholder, while
  // every other date field in the app is a native picker
  // (components/papers/PaperMetadataEditor.tsx). Free text matters here beyond
  // consistency: the filter re-runs 280ms after each keystroke and compares the
  // raw string against each paper's published date, so typing a date walked the
  // whole graph through its own prefixes, and a "05/01/2024" filtered silently
  // and wrongly. A date input's value is "" or a valid YYYY-MM-DD — the shape
  // the comparison already assumes.
  for (const id of ["filterDateFrom", "filterDateTo"]) {
    const tag = new RegExp(`<input\\b[^>]*\\bid="${id}"[^>]*>`).exec(html);
    assert.ok(tag, `graph.html must keep the ${id} input`);
    assert.match(tag[0], /\btype="date"/, `${id} must be a native date input`);
  }
});

test("graph.css styles every filter input type graph.html uses", () => {
  // The panel's inputs are styled through `input[type=...]` attribute
  // selectors, so a control whose type is not named falls back to the UA
  // default — a white box with black text inside the dark panel, which is
  // exactly what switching the Date range boxes to type=date would have done.
  const graphCss = fs.readFileSync(path.join(GRAPH_DIR, "graph.css"), "utf8");
  const types = new Set(
    [...html.matchAll(/<input\b[^>]*\btype="([a-z]+)"/g)].map((m) => m[1])
  );
  // checkbox and range are chrome the browser draws for us (`accent-color` and
  // `color-scheme` do the theming); the rest are boxes graph.css must paint.
  for (const type of types) {
    if (type === "checkbox" || type === "range") continue;
    assert.ok(
      graphCss.includes(`input[type=${type}]`),
      `graph.html uses input[type=${type}] but graph.css never styles it`
    );
  }
});

test("every element graph.js looks up by id exists in graph.html", () => {
  // graph.js is outside the bundler and its `$` is a bare getElementById, so a
  // renamed or deleted element is `null` and the first property access on it
  // throws — killing the whole script from wherever it happened to run, with
  // nothing on the canvas to say why. Only literal ids are checkable here; the
  // helpers that take an id parameter are driven by these same call sites.
  const declared = new Set(
    [...html.matchAll(/\bid="([^"]+)"/g)].map((m) => m[1])
  );
  const requested = new Set(
    [...graphJs.matchAll(/(?:\$|document\.getElementById)\(\s*'([\w-]+)'\s*\)/g)]
      .map((m) => m[1])
  );
  assert.ok(requested.size > 0, "the id sweep matched nothing — check the regex");
  for (const id of requested) {
    assert.ok(declared.has(id), `graph.js reads #${id}, which graph.html does not declare`);
  }

  // The collapse buttons name their body the same way, just through an
  // attribute instead of a call.
  for (const [, target] of html.matchAll(/\bdata-target="([^"]+)"/g)) {
    assert.ok(declared.has(target), `a panel toggle targets #${target}, which does not exist`);
  }
});

// ── Backend transport ───────────────────────────────────────────────────────
// The graph iframe is the only part of the app that reaches the backend by URL
// rather than through src/api/client.ts, so the two copies of "where is the
// backend" have to be pinned together. It used to sniff its own URL, which put
// it on the Vite dev server's database under `tauri dev` while every other
// surface read the in-process library; the host names the transport in the src
// now, and these guard both halves of that handshake.

const graphIframeSrcTs = fs.readFileSync(
  path.join(import.meta.dirname, "./graphIframeSrc.ts"),
  "utf8"
);
const papersTs = fs.readFileSync(path.join(import.meta.dirname, "../api/papers.ts"), "utf8");

test("graph.js accepts exactly the transports the host can name", () => {
  // A value the host starts sending and graph.js does not know falls through to
  // the URL sniff — silently, with no error anywhere, which is the failure mode
  // this whole channel exists to remove.
  const jsList = /const API_TRANSPORTS = \[([^\]]*)\]/.exec(graphJs);
  assert.ok(jsList, "graph.js must declare its accepted transports as a literal array");
  const guest = jsList![1].match(/'[a-z]+'/g)!.map((q) => q.slice(1, -1)).sort();

  const union = /export type GraphApiTransport =([^;]+);/.exec(graphIframeSrcTs);
  assert.ok(union, "graphIframeSrc.ts must declare GraphApiTransport as a literal union");
  const host = union![1].match(/"[a-z]+"/g)!.map((q) => q.slice(1, -1)).sort();

  assert.deepEqual(guest, host, "graph.js and graphIframeSrc.ts disagree on the transport vocabulary");
});

test("the graph reaches the custom scheme the same way the rest of the app does", () => {
  // Tauri serves it as http://linxiv.localhost on Windows and linxiv://localhost
  // everywhere else. src/api/papers.ts makes that split for the PDF routes and
  // graph.js has to make the identical one for its four GET endpoints — it
  // cannot import the helper, being outside the bundler.
  const bases = (src: string) =>
    [...src.matchAll(/["']((?:linxiv:\/\/|http:\/\/linxiv\.)[a-z.]*localhost)["']/g)]
      .map((m) => m[1])
      .sort();
  const host = bases(papersTs);
  assert.deepEqual(host, ["http://linxiv.localhost", "linxiv://localhost"]);
  assert.deepEqual(bases(graphJs), host, "graph.js names a different custom-scheme base than papers.ts");

  // ...and picks between them on the same test, so a Windows build doesn't get
  // one file's answer and one file's other.
  for (const src of [papersTs, graphJs]) {
    assert.match(src, /\/Windows\/i\.test\(navigator\.userAgent\)/);
  }
});

// ── /api/graph payload shape ────────────────────────────────────────────────
// The graph payload is the one wire shape in this app with no canonical Rust
// serializer: crates/core/src/graph.rs assembles it with `serde_json::json!`,
// so `#[derive(TS)]` has nothing to render and src/types/generated.ts cannot
// cover it. Both readers are therefore unchecked copies of it — the hand-written
// types in src/types/api.ts, and public/graph/graph.js, which lives outside the
// bundler entirely — and both had already drifted: api.ts declared a paper
// node's `id` as a string where graph.rs emits the bare SOURCE_FK integer and
// was missing eight of the fields the payload carries, while the `url` and `doi`
// graph.js copies onto every node still have no reader at all. A field added,
// renamed or dropped on the Rust side is silently `undefined` on both, so pin
// the three descriptions of the payload against each other.

const graphRs = fs.readFileSync(
  path.join(import.meta.dirname, "../../src-tauri/crates/core/src/graph.rs"),
  "utf8"
);
const graphRouteRs = fs.readFileSync(
  path.join(import.meta.dirname, "../../src-tauri/src/route/graph.rs"),
  "utf8"
);
const apiTs = fs.readFileSync(path.join(import.meta.dirname, "../types/api.ts"), "utf8");

/** Rust source with the `#[cfg(test)]` module (which builds its own `json!`s) cut off. */
function withoutTestModule(rust: string): string {
  const at = rust.indexOf("#[cfg(test)]");
  assert.ok(at > 0, "expected a #[cfg(test)] module to cut the fixtures off at");
  return rust.slice(0, at);
}

/** The body of every `json!({ … })` object literal, by brace matching. */
function jsonObjectBodies(rust: string): string[] {
  const bodies: string[] = [];
  for (const m of rust.matchAll(/json!\(\s*\{/g)) {
    let depth = 1;
    let i = m.index + m[0].length;
    const start = i;
    while (i < rust.length && depth > 0) {
      if (rust[i] === "{") depth++;
      else if (rust[i] === "}") depth--;
      i++;
    }
    assert.equal(depth, 0, "unbalanced json! object literal in graph.rs");
    bodies.push(rust.slice(start, i - 1));
  }
  return bodies;
}

/**
 * The keys of one object body. Only depth 0 counts, so the `"title"` in
 * `row.get::<_, Option<String>>("title")?` is a value, not a field.
 */
function jsonKeys(body: string): string[] {
  const keys: string[] = [];
  let depth = 0;
  for (let i = 0; i < body.length; i++) {
    const c = body[i];
    if (c === '"') {
      const end = body.indexOf('"', i + 1);
      assert.ok(end !== -1, "unterminated string in a json! object literal");
      if (depth === 0 && /^\s*:/.test(body.slice(end + 1))) keys.push(body.slice(i + 1, end));
      i = end;
    } else if (c === "{" || c === "[" || c === "(") depth++;
    else if (c === "}" || c === "]" || c === ")") depth--;
  }
  return keys;
}

const graphRsBodies = jsonObjectBodies(withoutTestModule(graphRs));

/** The one `json!` object carrying `"type": "<kind>"`, as a sorted field list. */
function rustNodeFields(kind: string): string[] {
  const tagged = graphRsBodies.filter((b) =>
    new RegExp(`"type"\\s*:\\s*"${kind}"`).test(b)
  );
  assert.equal(tagged.length, 1, `expected exactly one ${kind} node literal in graph.rs`);
  return jsonKeys(tagged[0]).sort();
}

/**
 * `want`, sorted — after asserting graph.rs still builds at least one `json!`
 * object with exactly those keys. The untyped shapes (edges, the envelopes) have
 * no `"type"` discriminator to find them by, so they are located by key set and
 * this only confirms the set is still there; `every json! object in graph.rs is
 * one this guard describes` below is what stops a NEW shape slipping past.
 */
function rustObjectWithKeys(want: string[], what: string): string[] {
  const sorted = [...want].sort();
  const hits = graphRsBodies.filter((b) => jsonKeys(b).sort().join() === sorted.join());
  assert.ok(hits.length > 0, `graph.rs builds no ${what} with the keys ${sorted.join(", ")}`);
  return sorted;
}

/** Fields declared by one `export interface`, by brace matching over its body. */
function tsInterfaceFields(ts: string, name: string): string[] {
  const at = ts.indexOf(`export interface ${name} {`);
  assert.ok(at !== -1, `src/types/api.ts declares no ${name}`);
  const open = ts.indexOf("{", at);
  let depth = 0;
  let i = open;
  for (; i < ts.length; i++) {
    if (ts[i] === "{") depth++;
    else if (ts[i] === "}" && --depth === 0) break;
  }
  const fields = [...ts.slice(open + 1, i).matchAll(/^\s*([a-z_]+)\??:/gm)].map((m) => m[1]);
  assert.ok(fields.length > 0, `${name} declares no fields`);
  return fields.sort();
}

test("src/types/api.ts declares the /api/graph nodes graph.rs actually builds", () => {
  // `project_ids` is not in the paper literal: augmented_graph_data decorates
  // each paper node with it after graph_data has emitted the node.
  const decorations = [...withoutTestModule(graphRs).matchAll(/\bnode\["(\w+)"\]\s*=/g)];
  const paperLoop = graphRs.indexOf("in paper_nodes");
  assert.ok(paperLoop > 0, "graph.rs no longer decorates the paper nodes in a `paper_nodes` loop");
  for (const m of decorations) {
    assert.ok(
      m.index > paperLoop,
      `graph.rs sets node["${m[1]}"] outside the paper loop — this guard would file it ` +
        "under the paper node anyway"
    );
  }

  assert.deepEqual(
    [...rustNodeFields("paper"), ...decorations.map((m) => m[1])].sort(),
    tsInterfaceFields(apiTs, "GraphPaperNode")
  );
  assert.deepEqual(rustNodeFields("author"), tsInterfaceFields(apiTs, "GraphAuthorNode"));
  assert.deepEqual(rustNodeFields("tag"), tsInterfaceFields(apiTs, "GraphTagNode"));

  // Nothing but those three is a node, so the union has to list all three.
  const union = /export type GraphNode =([^;]+);/.exec(apiTs);
  assert.ok(union, "api.ts must declare GraphNode as a union of the node shapes");
  assert.deepEqual(
    (union[1].match(/Graph\w+Node/g) ?? []).sort(),
    ["GraphAuthorNode", "GraphPaperNode", "GraphTagNode"]
  );
});

test("src/types/api.ts declares the /api/graph edges and envelope graph.rs builds", () => {
  assert.deepEqual(
    rustObjectWithKeys(["source", "target"], "graph edge"),
    tsInterfaceFields(apiTs, "GraphEdge")
  );
  assert.deepEqual(
    rustObjectWithKeys(["nodes", "edges"], "graph envelope"),
    tsInterfaceFields(apiTs, "GraphData")
  );
});

test("src/types/api.ts declares the /api/graph/project-options payload", () => {
  assert.deepEqual(
    rustObjectWithKeys(["id", "name", "color", "tags"], "project filter option"),
    tsInterfaceFields(apiTs, "GraphProjectOption")
  );
  // The list itself is wrapped by the route, not by graph.rs.
  const envelope = jsonObjectBodies(withoutTestModule(graphRouteRs));
  assert.deepEqual(
    envelope.map((b) => jsonKeys(b).sort().join()),
    ["projects"],
    "route/graph.rs wraps the options in a different envelope than api.ts declares"
  );
  assert.deepEqual(tsInterfaceFields(apiTs, "GraphProjectOptions"), ["projects"]);
});

test("every json! object in graph.rs is one this guard describes", () => {
  // Without this, a NEW shape added to the payload passes every check above by
  // simply never being looked at — which is exactly the drift they exist to
  // catch. Each body must fall into one of the five known buckets.
  const known = (body: string) => {
    const keys = jsonKeys(body).sort().join();
    return (
      /"type"\s*:\s*"(paper|author|tag)"/.test(body) ||
      keys === "source,target" ||
      keys === "edges,nodes" ||
      keys === "color,id,name,tags"
    );
  };
  const strays = graphRsBodies.filter((b) => !known(b)).map((b) => jsonKeys(b).join(", "));
  assert.deepEqual(strays, [], "graph.rs builds a payload shape nothing here describes");
});

test("the graph iframe carries every field /api/graph sends onto its nodes", () => {
  // graph.js copies the payload into cytoscape element data once, in loadGraph;
  // a field it does not name there is unreachable from every filter, tooltip
  // and click handler downstream, with nothing to say so.
  const start = graphJs.indexOf("const cyElements = [");
  const end = graphJs.indexOf("cy = cytoscape({");
  assert.ok(start !== -1 && end > start, "graph.js no longer builds a cyElements array");
  const elements = graphJs.slice(start, end);
  const edgeAt = elements.indexOf("...edges.map(");
  assert.ok(edgeAt > 0, "graph.js no longer maps the edge list inside cyElements");

  const read = (src: string, receiver: string) =>
    [...new Set([...src.matchAll(new RegExp(`\\b${receiver}\\.([a-z_]+)\\b`, "g"))].map((m) => m[1]))].sort();

  assert.deepEqual(
    read(elements.slice(0, edgeAt), "n"),
    [...new Set([...rustNodeFields("paper"), ...rustNodeFields("author"), ...rustNodeFields("tag"), "project_ids"])].sort(),
    "graph.js and graph.rs disagree on the node fields — a field on one side only is undefined on the other"
  );
  assert.deepEqual(
    read(elements.slice(edgeAt), "e"),
    rustObjectWithKeys(["source", "target"], "graph edge"),
    "graph.js and graph.rs disagree on the edge fields"
  );
});

test("the graph iframe reads the project options graph.rs sends", () => {
  // The Projects / Project Tags rows resolve through this list: a `color` that
  // stopped arriving leaves every swatch on the fallback accent, and a renamed
  // `tags` silently makes every Project Tags row match nothing.
  const at = graphJs.indexOf("_listOrNull(projData");
  assert.ok(at !== -1, "graph.js no longer maps the project-options payload");
  const mapper = graphJs.slice(at, graphJs.indexOf("))", at));
  assert.deepEqual(
    [...new Set([...mapper.matchAll(/\bp\.([a-z_]+)\b/g)].map((m) => m[1]))].sort(),
    rustObjectWithKeys(["id", "name", "color", "tags"], "project filter option")
  );
  assert.match(mapper, /projData,\s*'projects'/, "graph.js reads a different envelope key than the route sends");
});

// ── The reserved reading-list marker ────────────────────────────────────────
// A reading list IS a project carrying READING_LIST_TAG, so the marker is
// bookkeeping rather than a tag anyone typed. `list_tags` keeps it out of
// `/api/tags`, ProjectDetailPage refuses to let it be added by hand, and both
// surfaces that draw a project's tags filter it out of the chips. But
// `/api/graph/project-options` forwards each project's project_tags raw, which
// left the graph's Project Tags datalist as the one dropdown in the app
// offering it. graph.js lives outside the bundler and cannot import the
// constant, so its copy is pinned here.

const readingStatusTs = fs.readFileSync(
  path.join(import.meta.dirname, "./readingStatus.ts"),
  "utf8"
);

test("the graph hides the same reading-list marker the rest of the app does", () => {
  const host = /export const READING_LIST_TAG = "([^"]+)"/.exec(readingStatusTs);
  assert.ok(host, "readingStatus.ts must declare READING_LIST_TAG as a string literal");

  const guest = /const READING_LIST_TAG = '([^']+)'/.exec(graphJs);
  assert.ok(guest, "graph.js must declare its copy of READING_LIST_TAG as a string literal");
  assert.equal(
    guest![1],
    host![1],
    "graph.js and readingStatus.ts disagree on the reserved reading-list tag"
  );

  // Both halves fold before comparing — PROJECT_TAG labels are stored as typed,
  // so a project tagged "Reading-List" is the same marker.
  assert.match(readingStatusTs, /toLowerCase\(\) === READING_LIST_TAG/);
  assert.match(graphJs, /toLowerCase\(\) !== READING_LIST_TAG/);
});
