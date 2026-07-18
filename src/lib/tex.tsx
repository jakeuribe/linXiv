import type { ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { mathjax } from "mathjax-full/js/mathjax.js";
import { TeX } from "mathjax-full/js/input/tex.js";
import { SVG } from "mathjax-full/js/output/svg.js";
import { liteAdaptor } from "mathjax-full/js/adaptors/liteAdaptor.js";
import { RegisterHTMLHandler } from "mathjax-full/js/handlers/html.js";
import { AllPackages } from "mathjax-full/js/input/tex/AllPackages.js";
import { getSettings } from "../api/settings";
import type { Settings } from "../types/api";

// MathJax SVG pipeline, set up once. liteAdaptor renders to an HTML string we
// inject; fontCache "none" inlines glyph paths per container.
const adaptor = liteAdaptor();
RegisterHTMLHandler(adaptor);
const svgOutput = new SVG({ fontCache: "none" });
// Exclude packages that emit raw HTML nodes (html) or persist macro definitions
// across renders on the shared mjDoc (newcommand, configmacros). require/setoptions
// are excluded too: \require{html} would re-enable the filtered-out html extension
// (\href, \style, …) and reopen the raw-HTML XSS path.
const excludedPackages = new Set([
  "html",
  "require",
  "setoptions",
  "newcommand",
  "configmacros",
]);
const mathPackages = AllPackages.filter((p) => !excludedPackages.has(p));
const mjDoc = mathjax.document("", {
  InputJax: new TeX({ packages: mathPackages }),
  OutputJax: svgOutput,
});

// MathJax's container stylesheet (display/overflow/direction rules) is added to
// the document once; the SVG glyphs themselves use fill=currentColor.
if (typeof document !== "undefined" && !document.head.querySelector("[data-mathjax]")) {
  const css = adaptor.textContent(
    svgOutput.styleSheet(mjDoc) as Parameters<typeof adaptor.textContent>[0],
  );
  if (css) {
    const style = document.createElement("style");
    style.setAttribute("data-mathjax", "");
    style.textContent = css;
    document.head.appendChild(style);
  }
}

const selectTexEnabled = (s: Settings) => s.tex_rendering_enabled !== false;

export function useTexEnabled(): boolean {
  const { data } = useQuery({
    queryKey: ["settings"],
    queryFn: getSettings,
    select: selectTexEnabled,
  });
  return data ?? true;
}

// $$...$$ (display) or $...$ (inline). Inline requires a non-space just inside
// each delimiter: (?!\s) guards the open $ and the trailing [^$\s] guards the
// close $. An unpaired opener like the first $ in "$5 and $6" matches nothing;
// a paired "$5$" still renders as math. Inline is kept to a single line ([^$\n])
// so two stray `$` on consecutive lines (currency, $VAR) don't merge into math;
// display $$…$$ may still span lines.
const MATH_RE = /\$\$([\s\S]+?)\$\$|\$(?!\s)([^$\n]*?[^$\s])\$/g;

function mathHtml(tex: string, display: boolean): string | null {
  if (!tex.trim()) return null;
  try {
    const html = adaptor.outerHTML(mjDoc.convert(tex, { display }));
    mjDoc.clear(); // drop the stored node so mjDoc.math doesn't grow per render
    return html;
  } catch {
    return null;
  }
}

function toNodes(text: string, forceInline: boolean): ReactNode[] {
  const nodes: ReactNode[] = [];
  let last = 0;
  let key = 0;
  for (const m of text.matchAll(MATH_RE)) {
    if (m.index > last) nodes.push(text.slice(last, m.index));
    const isDisplay = m[1] !== undefined;
    const display = isDisplay && !forceInline;
    const tex = (isDisplay ? m[1] : m[2]) as string;
    const html = mathHtml(tex, display);
    if (html === null) {
      nodes.push(m[0]); // empty or unrenderable — keep the raw delimited source
    } else {
      nodes.push(<span key={key++} dangerouslySetInnerHTML={{ __html: html }} />);
    }
    last = m.index + m[0].length;
  }
  if (last < text.length) nodes.push(text.slice(last));
  return nodes;
}

// Render a string, turning $…$ / $$…$$ spans into MathJax SVG. When TeX is
// disabled or the string has no `$`, the raw text is returned unchanged.
// forceInline renders display math inline so callers inside line-clamp spans
// don't get a display:block container promoted out of the inline box.
export function MathText({
  children,
  forceInline = false,
}: {
  children: string | null | undefined;
  forceInline?: boolean;
}) {
  const enabled = useTexEnabled();
  const text = children ?? "";
  if (!enabled || !text.includes("$")) return <>{text}</>;
  return <>{toNodes(text, forceInline)}</>;
}
