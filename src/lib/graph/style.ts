// The cytoscape stylesheet, resolved from the app's live theme.
//
// The graph used to be a separate document with its own copy of the palette,
// which went stale silently every time src/lib/theme.ts gained a preset. It is
// part of the bundle now, so it reads `ThemeColors` directly — the same value
// every other surface in the app is painted from.

import type { StylesheetJson } from "cytoscape";
import type { ThemeColors } from "../theme.ts";

/**
 * Authors get no colour of their own because theme.ts has no fourth semantic
 * token. Node type is also encoded by SHAPE (paper = ellipse, author = diamond,
 * tag = roundrectangle), so this fixed hue only has to read as "not the accent".
 */
export const AUTHOR_COLOR = "#e8a838";

/** The app's own stack, from src/styles/globals.css. Cytoscape wants a bare CSS
 *  font-family string, so no quoting. */
export const LABEL_FONT_FAMILY = "Inter";
export const LABEL_FONT = `${LABEL_FONT_FAMILY}, system-ui, sans-serif`;
/** Longest a stalled webfont request may hold up the first render. */
export const FONT_LOAD_TIMEOUT_MS = 3000;

export const DIM_OPACITY = 0.08; // filter dim (isolate / non-matching)
export const SEL_DIM_OPACITY = 0.28; // softer dim for non-selected nodes
export const FULL_OPACITY = 1;

export const MIN_ZOOM = 0.05;
export const MAX_ZOOM = 10;

export function paperColor(t: ThemeColors): string {
  return t.accent;
}
export function tagColor(t: ThemeColors): string {
  return t.success;
}
export function highlightColor(t: ThemeColors): string {
  return t.danger;
}

export function graphStylesheet(t: ThemeColors): StylesheetJson {
  return [
    {
      selector: 'node[type = "paper"]',
      style: {
        shape: "ellipse",
        width: 20,
        height: 20,
        "background-color": paperColor(t),
        label: "data(label)",
        "font-size": 13,
        "font-weight": 600,
        // Theme text with a background-coloured halo over edges/nodes.
        color: t.text,
        "text-outline-color": t.bg,
        "text-outline-width": 2.5,
        "text-outline-opacity": 1,
        // Stop rendering labels below ~7px on-screen.
        "min-zoomed-font-size": 7,
        "font-family": LABEL_FONT,
        "text-valign": "center",
        "text-halign": "right",
        "text-margin-x": 8,
        "text-max-width": "180px",
        "text-wrap": "ellipsis",
        "border-width": 1.5,
        "border-color": t.bg,
      },
    },
    {
      selector: 'node[type = "author"]',
      style: {
        shape: "diamond",
        width: 14,
        height: 14,
        "background-color": AUTHOR_COLOR,
        label: "data(label)",
        "font-size": 12,
        "font-weight": 600,
        color: t.text,
        "text-outline-color": t.bg,
        "text-outline-width": 2.5,
        "text-outline-opacity": 1,
        "min-zoomed-font-size": 7,
        "font-family": LABEL_FONT,
        "text-valign": "center",
        "text-halign": "right",
        "text-margin-x": 7,
        "text-max-width": "140px",
        "text-wrap": "ellipsis",
      },
    },
    {
      selector: 'node[type = "tag"]',
      style: {
        shape: "round-rectangle",
        width: "label",
        height: 20,
        // A single length, NOT the CSS `0 7px` shorthand this was ported as.
        // cytoscape's `padding` is one `sizeMaybePercent`; a two-token string
        // misses its unit regex and then falls through to `parseFloat`, which
        // reads "0 7px" as 0 and accepts it silently — no warning, and every tag
        // chip drawn hard against its own label, since `width: "label"` makes the
        // chip exactly as wide as the text. The explicit height above is what
        // keeps this from padding the chip vertically too.
        padding: "7px",
        "background-color": tagColor(t),
        label: "data(label)",
        "font-size": 12,
        "font-weight": 600,
        // Label sits inside the chip: white text over a neutral scrim, so it
        // stays readable whatever hue --color-success resolves to.
        color: "#ffffff",
        "text-outline-color": "rgba(0,0,0,0.55)",
        "text-outline-width": 1.5,
        "text-outline-opacity": 1,
        "min-zoomed-font-size": 7,
        "font-family": LABEL_FONT,
        "text-valign": "center",
        "text-halign": "center",
        "border-width": 0,
      },
    },
    {
      selector: "edge",
      style: {
        width: 1.5,
        "line-color": t.border,
        "curve-style": "haystack",
      },
    },
  ] as StylesheetJson;
}

/**
 * Cytoscape measures every node label on an offscreen canvas with `ctx.font` and
 * caches the result on the RENDERER, keyed by the label text plus the font style
 * properties — not by whether the family had actually arrived. Inter is a
 * self-hosted webfont, so on a cold load the labels can all be measured in the
 * fallback face and keep those widths for the rest of the session: tag chips
 * (`width: 'label'`) come out the wrong size and `text-max-width` ellipsizes at
 * the wrong point. Reinstalling the stylesheet later does NOT help, so the fix
 * is to have the face in hand before the first render.
 */
export function whenLabelFontReady(): Promise<void> {
  const fonts = typeof document === "undefined" ? null : document.fonts;
  if (!fonts?.load) return Promise.resolve();
  // Never let a stalled font request hold the canvas hostage: past the timeout
  // the graph draws in the fallback face rather than not at all.
  return new Promise<void>((resolve) => {
    const timer = setTimeout(resolve, FONT_LOAD_TIMEOUT_MS);
    const done = () => {
      clearTimeout(timer);
      resolve();
    };
    fonts.load(`600 13px ${LABEL_FONT_FAMILY}`).then(done, done);
  });
}

/**
 * The opacity one element is painted at.
 *
 * Filtered out → the filter dim (0 under isolate); selected → full; visible but
 * unselected while something IS selected → a softer dim; otherwise full.
 */
export function opacityFor(
  filterVisible: boolean,
  selected: boolean,
  anySelected: boolean,
  isolate: boolean
): number {
  if (!filterVisible) return isolate ? 0 : DIM_OPACITY;
  if (selected) return FULL_OPACITY;
  return anySelected ? SEL_DIM_OPACITY : FULL_OPACITY;
}

/**
 * Cytoscape decides what a click can land on from `events` / `visibility` /
 * `display` and never from opacity, so an element the isolate filter has taken
 * to opacity 0 stays fully hit-testable: tapping apparently blank canvas
 * navigated to a paper the user could not see, dragged an invisible node, and
 * swallowed the background tap that clears the selection. Tie interactivity to
 * visibility instead.
 */
export function eventsFor(opacity: number): "yes" | "no" {
  return opacity === 0 ? "no" : "yes";
}
