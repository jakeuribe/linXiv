import { useState, useRef, useEffect } from "react";
import { useLocation, useNavigate } from "react-router";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Document, Page, pdfjs } from "react-pdf";
import type { PDFDocumentProxy } from "pdfjs-dist";
import { fetchArxiv } from "../api/search";
import { appendSavedId } from "../api/searchState";
import { apiFetch, bytesToBase64, isTauri } from "../api/client";
import { getPdfProxyUrl } from "../api/papers";
import { Button } from "../components/ui/button";
import { Spinner } from "../components/ui/spinner";
import type { SearchResult } from "../types/api";
import { isArxivId } from "../lib/papers";
import { MathText } from "../lib/tex";
import { invalidatePaperMutationQueries } from "../lib/paperMutations";
import { errText } from "../lib/errText";

pdfjs.GlobalWorkerOptions.workerSrc = new URL(
  "pdfjs-dist/build/pdf.worker.min.mjs",
  import.meta.url,
).toString();

interface PdfPreviewState {
  result: SearchResult;
  isSaved: boolean;
}

function isValidPdfPreviewState(state: unknown): state is PdfPreviewState {
  if (!state || typeof state !== "object") return false;
  const s = state as Record<string, unknown>;
  if (!s.result || typeof s.result !== "object") return false;
  const r = s.result as Record<string, unknown>;
  return (
    typeof r.source_id === "string" &&
    typeof r.title === "string" &&
    typeof r.paper_url === "string" &&
    typeof s.isSaved === "boolean"
  );
}

export default function PdfPreviewPage() {
  const navigate = useNavigate();
  const location = useLocation();
  const queryClient = useQueryClient();

  const state = isValidPdfPreviewState(location.state) ? location.state : null;
  const [saved, setSaved] = useState(state?.isSaved ?? false);
  const [numPages, setNumPages] = useState(0);
  const [useProxy, setUseProxy] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);
  const [containerWidth, setContainerWidth] = useState(0);
  const pdfDocRef = useRef<PDFDocumentProxy | null>(null);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const obs = new ResizeObserver((entries) => {
      setContainerWidth(entries[0].contentRect.width);
    });
    obs.observe(el);
    return () => obs.disconnect();
  }, []);

  const saveMutation = useMutation({
    mutationFn: async (sourceId: string) => {
      const result = await fetchArxiv(sourceId, true);
      if (pdfDocRef.current) {
        try {
          const bytes = await pdfDocRef.current.getData();
          const path = `/api/papers/${encodeURIComponent(sourceId)}/pdf`;
          if (isTauri) {
            await apiFetch(path, { method: "PUT", body: JSON.stringify({ file_b64: bytesToBase64(bytes) }) });
          } else {
            const form = new FormData();
            form.append("file", new Blob([bytes.slice()], { type: "application/pdf" }), `${sourceId}.pdf`);
            await apiFetch(path, { method: "PUT", body: form });
          }
        } catch (e) {
          console.error("PDF attach failed (non-fatal):", e);
        }
      }
      return result;
    },
    onSuccess: (data) => {
      if (data.saved) {
        setSaved(true);
        appendSavedId(data.source_id).catch((e) =>
          console.error("appendSavedId failed:", e)
        );
        invalidatePaperMutationQueries(queryClient);
      }
    },
  });

  if (!state) {
    return (
      <div className="flex items-center justify-center gap-2 h-full text-muted text-sm">
        No PDF selected.
        <button
          type="button"
          className="text-accent hover:underline focus:outline-none focus-visible:ring-2 focus-visible:ring-accent rounded"
          onClick={() => navigate("/search")}
        >
          Go to Search
        </button>
      </div>
    );
  }

  const { result } = state;
  const pdfSrc = useProxy ? getPdfProxyUrl(result.paper_url) : result.paper_url;

  return (
    <div className="flex flex-col h-full">
      <div
        className="shrink-0 flex items-center gap-3 px-4 py-3 border-b border-border"
        style={{ background: "var(--color-bg)" }}
      >
        <Button variant="ghost" size="sm" onClick={() => navigate(-1)}>
          ← Back
        </Button>

        <h1
          className="flex-1 text-sm font-medium text-text truncate"
          title={result.title}
        >
          <MathText forceInline>{result.title}</MathText>
        </h1>

        {isArxivId(result.source_id) && (
          saved ? (
            <span
              className="text-xs px-2 py-0.5 rounded-full font-medium"
              style={{
                background: "color-mix(in srgb, var(--color-success) 15%, transparent)",
                color: "var(--color-success)",
              }}
            >
              In library
            </span>
          ) : (
            <Button
              variant="primary"
              size="sm"
              disabled={saveMutation.isPending}
              onClick={() => saveMutation.mutate(result.source_id)}
            >
              {saveMutation.isPending ? (
                <span className="flex items-center gap-1.5">
                  <Spinner size={12} /> Saving…
                </span>
              ) : (
                "Save to library"
              )}
            </Button>
          )
        )}

        {saveMutation.isError && (
          <span className="text-xs text-danger">
            {errText(saveMutation.error, "Save failed")}
          </span>
        )}
      </div>

      {result.paper_url ? (
        <div ref={containerRef} className="flex-1 overflow-y-auto bg-[#525659]">
          <Document
            file={pdfSrc}
            onLoadSuccess={(pdf) => { setNumPages(pdf.numPages); pdfDocRef.current = pdf; }}
            loading={
              <div className="flex items-center justify-center gap-2 py-16 text-white/60 text-sm">
                <Spinner size={16} /> Loading PDF…
              </div>
            }
            error={
              <div className="flex flex-col items-center justify-center gap-3 py-16 text-sm">
                {!useProxy ? (
                  <>
                    <span className="text-white/60">
                      Could not load PDF directly (CORS).
                    </span>
                    <Button
                      variant="primary"
                      size="sm"
                      onClick={() => { setNumPages(0); setUseProxy(true); }}
                    >
                      Load via proxy
                    </Button>
                  </>
                ) : (
                  <>
                    <span className="text-danger">Failed to load PDF.</span>
                    <a
                      href={result.paper_url}
                      target="_blank"
                      rel="noreferrer"
                      className="text-accent hover:underline"
                    >
                      Open in browser
                    </a>
                  </>
                )}
              </div>
            }
          >
            {Array.from({ length: numPages }, (_, i) => (
              <Page
                key={i + 1}
                pageNumber={i + 1}
                width={containerWidth ? containerWidth - 32 : undefined}
                className="mx-auto my-2 shadow-md"
                renderTextLayer
                renderAnnotationLayer
              />
            ))}
          </Document>
        </div>
      ) : (
        <div className="flex-1 flex items-center justify-center text-muted text-sm">
          No PDF URL available for this paper.
        </div>
      )}
    </div>
  );
}
