import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Document, Page, pdfjs } from "react-pdf";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Spinner } from "../ui/spinner";
import {
  getAnnotations,
  createAnnotation,
  updateAnnotation,
  deleteAnnotation,
} from "../../api/annotations";
import {
  HIGHLIGHT_COLORS,
  parseAnchor,
  selectionToAnchor,
} from "../../lib/pdfAnchor";
import { HighlightLayer, type PageHighlight } from "./HighlightLayer";
import { PagePill } from "./PagePill";
import { submitOnCtrlEnter } from "../../lib/submitShortcut";

pdfjs.GlobalWorkerOptions.workerSrc = new URL(
  "pdfjs-dist/build/pdf.worker.min.mjs",
  import.meta.url,
).toString();

interface PdfReaderProps {
  /** Fetchable PDF URL (linxiv:// scheme in Tauri, proxied path in dev). */
  file: string;
  sourceId: string;
  /** Paper version the rendered PDF belongs to; anchors are scoped to it. */
  version: number;
  /** When viewed within a project, scope created highlights to it so they
   *  export/share with the project; null/undefined creates library-scoped ones. */
  projectId?: number | null;
  /** Fallback link shown if the PDF fails to load. */
  errorUrl?: string | null;
}

// Render the current page plus this many neighbors on each side; the rest are
// spacers. Neighbors cover scroll momentum before onScroll re-centers the window.
const PAGE_WINDOW = 4;

// How long resize (ResizeObserver) activity must be quiet before the live
// scaleX preview is committed as a real react-pdf re-render.
const RESIZE_SETTLE_MS = 120;

// Horizontal padding around each rendered page, subtracted from the scroller's
// measured width to get the actual page width react-pdf renders at.
const PAGE_INSET = 32;

// Spacer height for unrendered pages, estimated from a letter/A4 aspect ratio so
// the scrollbar and offsets stay roughly right until the real page mounts.
function estPageHeight(width: number) {
  return width ? Math.round((width - PAGE_INSET) * 1.3) : 800;
}

interface SelToolbar {
  top: number;
  left: number;
}
interface ActivePopup {
  id: number;
  top: number;
  left: number;
  quote: string;
  comment: string;
}

// Saved-PDF reader with Zotero-style text-highlight annotations. Each highlight
// is an ANNOTATION row carrying an `anchor` (see lib/pdfAnchor) plus an optional
// written comment; they render as a per-page overlay and round-trip through the
// annotations API shared with the Annotations tab.
export function PdfReader({ file, sourceId, version, projectId, errorUrl }: PdfReaderProps) {
  const qc = useQueryClient();
  const [numPages, setNumPages] = useState(0);
  const [page, setPage] = useState(1);
  const [width, setWidth] = useState(0);
  const [selBar, setSelBar] = useState<SelToolbar | null>(null);
  const [popup, setPopup] = useState<ActivePopup | null>(null);
  const [draft, setDraft] = useState("");
  const [selError, setSelError] = useState(false);

  const scrollerRef = useRef<HTMLDivElement | null>(null);
  const pagesWrapRef = useRef<HTMLDivElement | null>(null);
  const rafRef = useRef<number | null>(null);
  const obsRef = useRef<ResizeObserver | null>(null);
  // Last width actually committed to react-pdf (vs. the live, uncommitted
  // ResizeObserver reading) so mid-drag ticks can be scaled instead of reflowed.
  const committedWidthRef = useRef(0);
  const resizeTimerRef = useRef<number | null>(null);

  // Key must match PaperDetailPage's annotations query so the overlay and the
  // Annotations tab share one cache entry rather than each fetching separately.
  const { data: annData } = useQuery({
    queryKey: ["annotations", sourceId, { allProjects: true }],
    queryFn: () => getAnnotations(sourceId, undefined, true),
    enabled: !!sourceId,
  });

  const byPage = useMemo(() => {
    const byPage = new Map<number, PageHighlight[]>();
    for (const a of annData?.annotations ?? []) {
      const anchor = parseAnchor(a.anchor);
      // Only this version's anchors: page layout (and thus coords) differ per version.
      if (!anchor || anchor.version !== version) continue;
      const list = byPage.get(anchor.page) ?? [];
      list.push({ id: a.id, anchor });
      byPage.set(anchor.page, list);
    }
    return byPage;
  }, [annData, version]);

  // id → written comment, for populating the popup when a highlight is clicked.
  const commentById = useMemo(() => {
    const m = new Map<number, string>();
    for (const a of annData?.annotations ?? []) m.set(a.id, a.comment);
    return m;
  }, [annData]);

  const createMut = useMutation({
    mutationFn: (v: { anchorJson: string; quote: string; top: number; left: number }) =>
      createAnnotation({
        source_id: sourceId,
        anchor: v.anchorJson,
        project_id: projectId ?? null,
      }),
    onSuccess: (data, v) => {
      qc.invalidateQueries({ queryKey: ["annotations"] });
      // Open the comment popup on the fresh highlight so a comment can be added
      // right away — a highlight and its comment are one gesture, not two screens.
      const pos = clampToViewport(v.left, v.top, 280, 220);
      setDraft("");
      updateMut.reset();
      deleteMut.reset();
      setPopup({ id: data.id, top: pos.top, left: pos.left, quote: v.quote, comment: "" });
    },
  });
  const updateMut = useMutation({
    mutationFn: (v: { id: number; comment: string }) =>
      updateAnnotation(v.id, v.comment),
    onSuccess: (_data, v) => {
      qc.invalidateQueries({ queryKey: ["annotations"] });
      // close only the popup we edited, never one reopened mid-flight
      setPopup((p) => (p?.id === v.id ? null : p));
    },
  });
  const deleteMut = useMutation({
    mutationFn: (id: number) => deleteAnnotation(id),
    onSuccess: (_data, id) => {
      qc.invalidateQueries({ queryKey: ["annotations"] });
      setPopup((p) => (p?.id === id ? null : p));
    },
  });

  useEffect(
    () => () => {
      if (rafRef.current !== null) cancelAnimationFrame(rafRef.current);
      if (resizeTimerRef.current !== null) {
        clearTimeout(resizeTimerRef.current);
        pagesWrapRef.current?.style.removeProperty("transform");
      }
      obsRef.current?.disconnect();
    },
    [],
  );

  // Dismiss the toolbar/popup on a mousedown outside the reader scroller.
  useEffect(() => {
    if (!selBar && !popup) return;
    const onDocDown = (e: MouseEvent) => {
      if (scrollerRef.current?.contains(e.target as Node)) return;
      setSelBar(null);
      setPopup(null);
    };
    document.addEventListener("mousedown", onDocDown);
    return () => document.removeEventListener("mousedown", onDocDown);
  }, [selBar, popup]);

  // Stable so React doesn't detach/reattach the ref (re-creating the observer)
  // on every render — e.g. each setPage during a scroll.
  const attachScroller = useCallback((el: HTMLDivElement | null) => {
    obsRef.current?.disconnect();
    if (resizeTimerRef.current !== null) {
      clearTimeout(resizeTimerRef.current);
      resizeTimerRef.current = null;
      pagesWrapRef.current?.style.removeProperty("transform");
    }
    scrollerRef.current = el;
    if (!el) return;
    const obs = new ResizeObserver((entries) => {
      const newWidth = entries[0].contentRect.width;
      // First measurement (mount): commit immediately, nothing to scale from yet.
      if (committedWidthRef.current === 0) {
        committedWidthRef.current = newWidth;
        setWidth(newWidth);
        return;
      }
      // Mid-drag ticks: cheaply scale the already-rendered pages via CSS instead
      // of reflowing react-pdf on every tick; commit the real width (and let
      // react-pdf re-render at full quality) once resizing settles. Pages render
      // at width-PAGE_INSET, so scale by the inset width, not the outer one;
      // "top center" keeps mx-auto-centered pages from sliding sideways.
      const wrap = pagesWrapRef.current;
      const prevInset = committedWidthRef.current - PAGE_INSET;
      if (wrap && prevInset > 0) {
        wrap.style.transform = `scaleX(${(newWidth - PAGE_INSET) / prevInset})`;
        wrap.style.transformOrigin = "top center";
      }
      if (resizeTimerRef.current !== null) clearTimeout(resizeTimerRef.current);
      resizeTimerRef.current = window.setTimeout(() => {
        resizeTimerRef.current = null;
        if (wrap) wrap.style.transform = "";
        committedWidthRef.current = newWidth;
        setWidth(newWidth);
      }, RESIZE_SETTLE_MS);
    });
    obs.observe(el);
    obsRef.current = obs;
  }, []);

  function goToPage(target: number) {
    if (numPages <= 0) return;
    const next = Math.min(Math.max(target, 1), numPages);
    setPage(next);
    const scroller = scrollerRef.current;
    const pageEl =
      scroller?.querySelectorAll<HTMLElement>(".pdf-page-slot")[next - 1];
    if (scroller && pageEl) scroller.scrollTop = pageEl.offsetTop;
  }

  function onScroll(e: React.UIEvent<HTMLDivElement>) {
    const scroller = e.currentTarget;
    if (rafRef.current !== null) return;
    rafRef.current = requestAnimationFrame(() => {
      rafRef.current = null;
      if (!scroller.isConnected) return;
      const pages = scroller.querySelectorAll<HTMLElement>(".pdf-page-slot");
      if (pages.length === 0) return;
      let nearest = 1;
      let best = Infinity;
      pages.forEach((el, i) => {
        const dist = Math.abs(el.offsetTop - scroller.scrollTop);
        if (dist < best) {
          best = dist;
          nearest = i + 1;
        }
      });
      setPage(nearest);
    });
    if (selBar) setSelBar(null);
    if (popup) setPopup(null);
  }

  // On mouse up, if there's a real text selection inside a page, surface the
  // color picker near the selection's end so a click commits the highlight.
  function onMouseUp() {
    const sel = window.getSelection();
    if (!sel || sel.isCollapsed || sel.rangeCount === 0) return;
    if (!sel.toString().trim()) return;
    const range = sel.getRangeAt(0);
    const startNode = range.startContainer;
    const startEl =
      startNode.nodeType === Node.ELEMENT_NODE
        ? (startNode as Element)
        : startNode.parentElement;
    if (!startEl?.closest(".react-pdf__Page")) return;
    const rects = range.getClientRects();
    const last = rects[rects.length - 1];
    if (!last) return;
    setSelBar(clampToViewport(last.left, last.bottom + 6, 170, 40));
  }

  function commitHighlight(color: string) {
    const anchor = selectionToAnchor(version, color);
    const bar = selBar;
    setSelBar(null);
    window.getSelection()?.removeAllRanges();
    if (!anchor) {
      setSelError(true);
      return;
    }
    setSelError(false);
    createMut.mutate({
      anchorJson: JSON.stringify(anchor),
      quote: anchor.quote,
      top: bar?.top ?? 120,
      left: bar?.left ?? 120,
    });
  }

  // A click (not a drag-select) is hit-tested against highlight rects on the
  // clicked page; a hit opens the comment popup. The visual overlay is
  // pointer-events:none, so this keeps highlighted text fully selectable.
  function onClick(e: React.MouseEvent<HTMLDivElement>) {
    const sel = window.getSelection();
    if (sel && !sel.isCollapsed && sel.toString().trim()) return; // was a selection
    const target = document.elementFromPoint(e.clientX, e.clientY);
    const pageEl = target?.closest<HTMLElement>(".react-pdf__Page");
    if (!pageEl) return;
    const pageNum = Number(pageEl.getAttribute("data-page-number"));
    const list = byPage.get(pageNum);
    if (!list || list.length === 0) return;
    const box = pageEl.getBoundingClientRect();
    if (box.width === 0 || box.height === 0) return;
    const nx = (e.clientX - box.left) / box.width;
    const ny = (e.clientY - box.top) / box.height;
    const hit = list.find(({ anchor }) =>
      anchor.rects.some(
        (r) => nx >= r.x && nx <= r.x + r.w && ny >= r.y && ny <= r.y + r.h,
      ),
    );
    if (!hit) return;
    const pos = clampToViewport(e.clientX, e.clientY + 6, 280, 220);
    const comment = commentById.get(hit.id) ?? "";
    setDraft(comment);
    updateMut.reset();
    deleteMut.reset();
    setPopup({
      id: hit.id,
      top: pos.top,
      left: pos.left,
      quote: hit.anchor.quote,
      comment,
    });
  }

  // Annotation was deleted, or its server comment moved past the popup's
  // open-time baseline (popup.comment), while the popup was open elsewhere.
  const popupStale = popup
    ? !annData?.annotations.some((a) => a.id === popup.id) ||
      (commentById.get(popup.id) ?? "") !== popup.comment
    : false;
  const canSaveComment = popup
    ? !updateMut.isPending && draft !== popup.comment && !popupStale
    : false;

  return (
    <div className="relative w-full h-full min-h-0 flex flex-col">
      <div
        ref={attachScroller}
        onScroll={onScroll}
        onMouseDown={() => {
          // starting a new gesture dismisses any open chrome
          if (selBar) setSelBar(null);
          if (popup) setPopup(null);
          if (selError) setSelError(false);
        }}
        onMouseUp={onMouseUp}
        onClick={onClick}
        className="w-full h-full overflow-y-auto bg-[#525659]"
      >
        <Document
          file={file}
          onLoadSuccess={(pdf) => setNumPages(pdf.numPages)}
          loading={
            <div className="flex items-center justify-center gap-2 py-16 text-white/60 text-sm">
              <Spinner size={16} /> Loading PDF…
            </div>
          }
          error={
            <div className="flex flex-col items-center justify-center gap-3 py-16 text-sm">
              <span className="text-danger">Failed to load PDF.</span>
              {errorUrl && (
                <a
                  href={errorUrl}
                  target="_blank"
                  rel="noreferrer"
                  className="text-accent hover:underline"
                >
                  Open in browser
                </a>
              )}
            </div>
          }
        >
          {/* Only pages within PAGE_WINDOW of the current page mount a canvas;
              the rest are fixed-height spacers so scroll offsets stay correct.
              Wrapped so a mid-drag resize can cheaply scaleX this whole group
              (see attachScroller) instead of reflowing every page. */}
          <div ref={pagesWrapRef}>
            {Array.from({ length: numPages }, (_, i) => {
              const pn = i + 1;
              const pageWidth = width ? width - PAGE_INSET : undefined;
              if (Math.abs(pn - page) > PAGE_WINDOW) {
                return (
                  <div
                    key={pn}
                    className="pdf-page-slot mx-auto my-2"
                    style={{ width: pageWidth, height: estPageHeight(width) }}
                  />
                );
              }
              return (
                <div key={pn} className="pdf-page-slot mx-auto my-2">
                  <Page
                    pageNumber={pn}
                    width={pageWidth}
                    className="shadow-md"
                    renderTextLayer
                    renderAnnotationLayer
                  >
                    <HighlightLayer highlights={byPage.get(pn) ?? []} />
                  </Page>
                </div>
              );
            })}
          </div>
        </Document>
      </div>

      <PagePill page={page} total={numPages} onGo={goToPage} />

      {selBar && (
        <div
          className="fixed z-30 flex items-center gap-1.5 rounded-full bg-panel border border-border shadow-card px-2 py-1.5"
          style={{ top: selBar.top, left: selBar.left }}
          // preventDefault keeps the text selection alive for commitHighlight;
          // stopPropagation keeps the container's mousedown/up from clearing or
          // re-opening this toolbar mid-click (which would cancel the swatch click).
          onMouseDown={(e) => {
            e.preventDefault();
            e.stopPropagation();
          }}
          onMouseUp={(e) => e.stopPropagation()}
        >
          {HIGHLIGHT_COLORS.map((c) => (
            <button
              key={c}
              aria-label={`Highlight ${c}`}
              onClick={() => commitHighlight(c)}
              className="w-4 h-4 rounded-full border border-black/20 hover:scale-110 transition-transform"
              style={{ backgroundColor: c }}
            />
          ))}
        </div>
      )}

      {popup && (
        <div
          className="fixed z-30 w-[280px] rounded-md bg-panel border border-border shadow-card p-2.5 flex flex-col gap-2"
          style={{ top: popup.top, left: popup.left }}
          onMouseDown={(e) => e.stopPropagation()}
          onMouseUp={(e) => e.stopPropagation()}
          onKeyDown={submitOnCtrlEnter(() => {
            if (canSaveComment) {
              updateMut.mutate({ id: popup.id, comment: draft });
            }
          })}
        >
          {popup.quote && (
            <p className="text-xs text-muted line-clamp-3 italic">
              “{popup.quote}”
            </p>
          )}
          <textarea
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            placeholder="Add a comment…"
            rows={3}
            autoFocus
            className="w-full resize-none rounded border border-border bg-surface2 px-2 py-1.5 text-xs text-text focus:outline-none focus:border-accent"
          />
          <div className="flex items-center justify-between">
            <button
              disabled={deleteMut.isPending || updateMut.isPending}
              onClick={() => deleteMut.mutate(popup.id)}
              className="text-xs font-medium text-[var(--color-danger)] hover:underline disabled:opacity-50"
            >
              {deleteMut.isPending ? "Deleting…" : "Delete"}
            </button>
            <button
              disabled={!canSaveComment}
              onClick={() => updateMut.mutate({ id: popup.id, comment: draft })}
              className="text-xs font-medium text-accent hover:underline disabled:opacity-40"
            >
              {updateMut.isPending ? "Saving…" : "Save"}
            </button>
          </div>
          {popupStale && (
            <p className="text-xs" style={{ color: "var(--color-danger)" }}>
              Comment was updated elsewhere — close to reload.
            </p>
          )}
        </div>
      )}

      {(createMut.isError || updateMut.isError || deleteMut.isError || selError) && (
        <div
          className="absolute bottom-16 left-1/2 -translate-x-1/2 z-30 rounded-md bg-panel border border-border shadow-card px-3 py-1.5 text-xs"
          style={{ color: "var(--color-danger)" }}
        >
          {selError
            ? "Couldn't capture that selection. Try selecting the text again."
            : createMut.isError
              ? "Couldn't save highlight. Try again."
              : updateMut.isError
                ? "Couldn't save comment. Try again."
                : "Couldn't delete highlight. Try again."}
        </div>
      )}
    </div>
  );
}

// Keep a fixed-positioned floater (toolbar/popup) on-screen near the right/bottom
// edges; w/h are the floater's approximate size.
function clampToViewport(left: number, top: number, w: number, h: number) {
  return {
    left: Math.max(8, Math.min(left, window.innerWidth - w)),
    top: Math.max(8, Math.min(top, window.innerHeight - h)),
  };
}

