// Framing the graph inside the strip the filter panels leave uncovered.
//
// cytoscape's own `cy.fit()` frames across the WHOLE canvas, and the panel
// column sits over its right edge, so a plain fit pushes the rightmost nodes —
// and their right-hand labels, which stick out further still — underneath the
// panels on every load, settle and reveal. This is cytoscape's own
// getFitViewport() math with the width narrowed to the visible strip.

export const FIT_PADDING = 40;

export interface BoundingBox {
  x1: number;
  y1: number;
  x2: number;
  y2: number;
  w: number;
  h: number;
}

export interface Viewport {
  zoom: number;
  pan: { x: number; y: number };
}

/**
 * The viewport that frames `bb` inside a `width`x`height` canvas whose rightmost
 * `gutter` pixels are covered, or `null` when the caller should fall back to
 * cytoscape's own fit (nothing is covered, the strip is too narrow to frame in,
 * or the box is degenerate). Returning null rather than a best effort is what
 * keeps the unfiltered, un-paneled case byte-identical to plain `cy.fit()`.
 */
export function fitViewport(
  bb: BoundingBox,
  width: number,
  height: number,
  gutter: number,
  zoomRange: { min: number; max: number },
  padding = FIT_PADDING
): Viewport | null {
  const available = width - gutter;
  if (gutter <= 0 || available <= 2 * padding) return null;
  if (!(bb.w > 0) || !(bb.h > 0)) return null;

  let zoom = Math.min((available - 2 * padding) / bb.w, (height - 2 * padding) / bb.h);
  // minZoom wins over maxZoom on a clash, exactly as cytoscape resolves it.
  zoom = Math.max(Math.min(zoom, zoomRange.max), zoomRange.min);
  if (!(zoom > 0)) return null;

  return {
    zoom,
    pan: {
      x: (available - zoom * (bb.x1 + bb.x2)) / 2,
      y: (height - zoom * (bb.y1 + bb.y2)) / 2,
    },
  };
}

/**
 * Keep a floating box (the hover inspector) beside `anchor` but inside the
 * canvas and clear of the panel column — the same gutter `fitViewport` frames
 * around. Flips to the other side of the anchor rather than sliding, so the box
 * never covers the node it describes.
 */
export function placeFloatingBox(
  anchor: { x: number; y: number },
  box: { width: number; height: number },
  canvas: { width: number; height: number; gutter: number },
  offset = 14,
  margin = 8
): { left: number; top: number } {
  const rightLimit = canvas.width - canvas.gutter - margin;
  let left = anchor.x + offset;
  let top = anchor.y + offset;
  if (canvas.width && left + box.width > rightLimit) left = anchor.x - offset - box.width;
  if (canvas.height && top + box.height > canvas.height - margin) {
    top = anchor.y - offset - box.height;
  }
  return { left: Math.max(margin, left), top: Math.max(margin, top) };
}
