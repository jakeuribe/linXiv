// Run: node --experimental-transform-types --test src/lib/graphIframeTheme.test.ts
//
// public/graph/graph.html paints before graph.js runs, so it needs the theme
// colours up front. It used to carry its own inlined copy of the PRESETS table
// from src/lib/theme.ts, which nothing kept in sync — the "Reading Room" preset
// was added to theme.ts and never to graph.html, so those users got a flash of
// Navy. The host now freezes getColors() into the iframe src instead
// (graphIframeSrc), and this file runs graph.html's real inline script against
// that real src to pin the contract.
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import { graphIframeSrc } from "./graphIframeSrc.ts";
import { PRESETS, getColors } from "./theme.ts";
import type { PresetName, ThemeMode } from "./theme.ts";

const GRAPH_HTML = new URL("../../public/graph/graph.html", import.meta.url);
const html = fs.readFileSync(GRAPH_HTML, "utf8");

// The pre-paint bootstrap is the first inline <script> in <head>.
function prePaintScript(): string {
  const open = html.indexOf("<script>");
  const close = html.indexOf("</script>", open);
  assert.ok(open !== -1 && close !== -1, "graph.html must keep its inline pre-paint script");
  return html.slice(open + "<script>".length, close);
}

// Run that script with graph.html loaded at `src`, returning the --color-* vars
// it set on documentElement.
function runPrePaint(src: string): Record<string, string> {
  const set: Record<string, string> = {};
  const ctx = vm.createContext({
    window: { location: { search: src.includes("?") ? src.slice(src.indexOf("?")) : "" } },
    document: { documentElement: { style: { setProperty: (k: string, v: string) => { set[k] = v; } } } },
    URLSearchParams,
    JSON,
  });
  vm.runInContext(prePaintScript(), ctx, { filename: "graph.html#prepaint" });
  return set;
}

const MODES: ThemeMode[] = ["dark", "light"];

test("every theme preset reaches the graph iframe unchanged", () => {
  for (const preset of Object.keys(PRESETS) as PresetName[]) {
    for (const mode of MODES) {
      const colors = getColors(preset, mode);
      const vars = runPrePaint(graphIframeSrc({ excludeSingleAuthors: false, api: "linxiv", mode, theme: colors }));
      for (const key of Object.keys(colors) as Array<keyof typeof colors>) {
        assert.equal(
          vars[`--color-${key}`],
          colors[key],
          `${preset}/${mode} ${key} must match src/lib/theme.ts, not a copy inside graph.html`
        );
      }
    }
  }
});

test("graph.html holds no palette of its own", () => {
  // A hardcoded colour here is a second source of truth that drifts silently:
  // theme.ts changes, graph.html does not, and only the iframe looks wrong.
  assert.deepEqual(html.match(/#[0-9a-fA-F]{3,8}\b/g), null);
});

test("graph.css holds no colour outside a var() fallback", () => {
  // Same drift, one file over: graph.css is allowed exactly one kind of hex —
  // the standalone-load fallback inside `var(--color-x, #hex)`. Anything else
  // is a colour the theme can no longer reach, which is how the OR toggle ended
  // up amber-on-Navy and the remove button stuck on the Navy danger red.
  const css = fs.readFileSync(new URL("../../public/graph/graph.css", import.meta.url), "utf8");
  // Collapse `var(--x, <anything without parens>)` until nothing is left to
  // collapse, so nested/`color-mix()`-wrapped fallbacks are stripped too.
  let stripped = css;
  for (let prev = ""; prev !== stripped; ) {
    prev = stripped;
    stripped = stripped.replace(/var\(\s*--[a-z0-9-]+\s*,[^()]*?\)/g, "var()");
  }
  assert.deepEqual(stripped.match(/#[0-9a-fA-F]{3,8}\b/g), null);
});

test("graph.js reads every theme token graph.html can set", () => {
  // The pre-paint VARS list and graph.js's THEME_VARS drive the same custom
  // properties from two files; a token added to one and not the other is set
  // before paint and then dropped on the first theme_update (or vice versa).
  const js = fs.readFileSync(new URL("../../public/graph/graph.js", import.meta.url), "utf8");
  const jsVars = /const THEME_VARS = \[([^\]]*)\]/.exec(js);
  const htmlVars = /var VARS = \[([^\]]*)\]/s.exec(html);
  assert.ok(jsVars && htmlVars, "both files must declare their token list as a literal array");
  const parse = (m: RegExpExecArray) => m[1].match(/'[a-z0-9]+'/g)!.map((q) => q.slice(1, -1)).sort();
  const tokens = parse(jsVars!);
  assert.deepEqual(tokens, parse(htmlVars!));
  assert.deepEqual(tokens, Object.keys(getColors("Navy", "dark")).sort());
});

test("colour overrides and their alpha reach the iframe", () => {
  // getColors folds overrideAlphas into rgba(); the old localStorage bootstrap
  // read raw overrides and dropped the alpha entirely.
  const colors = getColors("Navy", "dark", { accent: "#ff0000" }, { accent: 50 });
  assert.match(colors.accent, /^rgba\(/);
  const vars = runPrePaint(graphIframeSrc({ excludeSingleAuthors: false, api: "linxiv", mode: "dark", theme: colors }));
  assert.equal(vars["--color-accent"], colors.accent);
});

test("the src carries the hide-single-authors option graph.js bootstraps from", () => {
  const theme = getColors("Navy", "dark");
  const on = new URL(graphIframeSrc({ excludeSingleAuthors: true, api: "linxiv", mode: "dark", theme }), "http://x");
  assert.equal(on.searchParams.get("excludeSingleAuthors"), "1");
  const off = new URL(graphIframeSrc({ excludeSingleAuthors: false, api: "linxiv", mode: "dark", theme }), "http://x");
  assert.equal(off.searchParams.get("excludeSingleAuthors"), null);
});

test("opened standalone, the iframe sets no vars and falls back to graph.css", () => {
  assert.deepEqual(runPrePaint("/graph/graph.html"), {});
  // Malformed input must not throw before the document paints.
  assert.deepEqual(runPrePaint("/graph/graph.html?theme=not-json"), {});
});

test("the src carries the light/dark mode as color-scheme", () => {
  // The eight ThemeColors tokens do not encode light vs dark, and the iframe is
  // a separate document that inherits none of tokens.css's `[data-mode]` rule.
  // With no value the guest sat on the UA default of `light`, so WebKitGTK drew
  // every native control in the filter panels — the Date range pickers, the
  // checkboxes, the force sliders, the #right-panels scrollbar — in the OS
  // light theme on top of a dark preset. tokens.css:13 documents that exact
  // failure for the host.
  for (const mode of MODES) {
    const src = graphIframeSrc({ excludeSingleAuthors: false, api: "linxiv", mode, theme: getColors("Navy", mode) });
    assert.equal(new URL(src, "http://x").searchParams.get("mode"), mode);
    assert.equal(runPrePaint(src)["color-scheme"], mode);
  }
});

test("the mode applies even when the palette does not", () => {
  // ?theme= used to `return` before anything else ran. A malformed palette
  // falls back to graph.css's var() defaults, which are the DARK ones — so it
  // must not take the scheme down with it.
  assert.equal(runPrePaint("/graph/graph.html?mode=light&theme=not-json")["color-scheme"], "light");
  // ...and an unrecognised mode sets nothing rather than an invalid declaration.
  assert.deepEqual(runPrePaint("/graph/graph.html?mode=sepia"), {});
});

test("graph.css names a color-scheme for a standalone load", () => {
  // Same contract one file over: opened outside the app there is no ?mode=, and
  // graph.css's var() fallbacks are the dark palette, so its own default has to
  // agree with them.
  const css = fs.readFileSync(new URL("../../public/graph/graph.css", import.meta.url), "utf8");
  assert.match(css, /:root\s*\{[^}]*color-scheme:\s*dark/);
});

test("graph.js accepts the same modes graph.html does", () => {
  // Pre-paint reads ?mode=; every later switch rides theme_update. A list that
  // drifts leaves the mode correct at load and frozen from then on.
  const js = fs.readFileSync(new URL("../../public/graph/graph.js", import.meta.url), "utf8");
  const jsModes = /const COLOR_SCHEMES = \[([^\]]*)\]/.exec(js);
  const htmlModes = /var MODES = \[([^\]]*)\]/s.exec(html);
  assert.ok(jsModes && htmlModes, "both files must declare their mode list as a literal array");
  const parse = (m: RegExpExecArray) => m[1].match(/'[a-z]+'/g)!.map((q) => q.slice(1, -1)).sort();
  assert.deepEqual(parse(jsModes!), parse(htmlModes!));
  assert.deepEqual(parse(jsModes!), [...MODES].sort());
});

test("the src names the backend transport graph.js bootstraps from", () => {
  // The guest cannot derive this: `tauri dev` and browser dev serve it from the
  // same http://localhost:5180, while the app around it reads the in-process
  // library in one case and a separate dev server in the other.
  const theme = getColors("Navy", "dark");
  for (const api of ["linxiv", "origin"] as const) {
    const src = graphIframeSrc({ excludeSingleAuthors: false, api, mode: "dark", theme });
    assert.equal(new URL(src, "http://x").searchParams.get("api"), api);
  }
});
