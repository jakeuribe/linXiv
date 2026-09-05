import { useState, useRef, useEffect, useCallback } from "react";
import { useParams, useNavigate, useLocation } from "react-router";
import { useQuery, useMutation, useQueryClient, keepPreviousData } from "@tanstack/react-query";
import { Document, Page } from "react-pdf";
import type { PDFDocumentProxy } from "pdfjs-dist";
import {
  getPaperBySfk,
  getPaperVersions,
  getPaperPdfUrl,
  getPdfProxyUrl,
  getDoiVersionCandidates,
  fetchFullText,
  mergePapers,
} from "../api/papers";
import { getNotes, deleteNote } from "../api/notes";
import { getAnnotations, deleteAnnotation, updateAnnotation } from "../api/annotations";
import { listProjects } from "../api/projects";
import { apiFetch, bytesToBase64, isTauri } from "../api/client";
import type { Note, Paper, Annotation } from "../types/api";
import { PdfReader } from "../components/pdf/PdfReader";
import { PagePill } from "../components/pdf/PagePill";
import { parseAnchor } from "../lib/pdfAnchor";
import {
  invalidateAnnotationQueries,
  invalidateNoteQueries,
  invalidatePaperMutationQueries,
  invalidatePaperQueries,
} from "../lib/paperMutations";
import { submitOnCtrlEnter } from "../lib/submitShortcut";
import { Spinner } from "../components/ui/spinner";
import { Button } from "../components/ui/button";
import { Badge } from "../components/ui/badge";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "../components/ui/tabs";
import { Card, MonoLabel } from "../components/ui/card";
import { NoteCard } from "../components/notes/NoteCard";
import { NoteEditor } from "../components/notes/NoteEditor";
import { PaperMetadataEditor } from "../components/papers/PaperMetadataEditor";
import { labelForSource } from "../lib/papers";
import { MathText } from "../lib/tex";
import { formatDate } from "../lib/date";
import { TagBadge } from "../components/tags/TagBadge";
import { openPdfInSystem } from "../api/pdfs";
import { remotePdfPath } from "../api/remote";
import { libraryFetch, useBackendStore } from "../stores/backend";
import { errText } from "../lib/errText";

const LATEST_VERSION_KEY = "latest" as const;

// Draggable split between the PDF pane and the details/notes pane. Persisted so
// the chosen width survives navigation/reload. Only active on lg screens where
// the two panes sit side by side (below lg they stack vertically).
const RIGHT_PANE_KEY = "paperDetail.rightPaneWidth";
const MIN_RIGHT_PANE = 300;
const MAX_RIGHT_PANE = 720;
const DEFAULT_RIGHT_PANE = 388;

export default function PaperDetailPage() {
  const { sfk } = useParams<{ sfk: string }>();
  const navigate = useNavigate();
  const location = useLocation();
  const queryClient = useQueryClient();

  const [showAddNote, setShowAddNote] = useState(false);
  const [editingNoteId, setEditingNoteId] = useState<number | null>(null);
  const [showEditor, setShowEditor] = useState(false);
  const [openNativeError, setOpenNativeError] = useState<string | null>(null);
  const [openNativeLoading, setOpenNativeLoading] = useState(false);
  const openNativeAbortRef = useRef<AbortController | null>(null);
  // null means "latest"; a number means a specific stored version
  const [selectedVersion, setSelectedVersion] = useState<number | null>(null);

  // Resizable right (details/notes) pane width.
  const [rightWidth, setRightWidth] = useState(() => {
    const saved = Number(localStorage.getItem(RIGHT_PANE_KEY));
    return saved >= MIN_RIGHT_PANE && saved <= MAX_RIGHT_PANE ? saved : DEFAULT_RIGHT_PANE;
  });
  const [isWide, setIsWide] = useState(
    () => typeof window !== "undefined" && window.matchMedia("(min-width: 1024px)").matches,
  );
  const twoPaneRef = useRef<HTMLDivElement>(null);
  const draggingRef = useRef(false);
  const dividerRafRef = useRef<number | null>(null);

  useEffect(() => {
    const mq = window.matchMedia("(min-width: 1024px)");
    const onChange = (e: MediaQueryListEvent) => setIsWide(e.matches);
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, []);

  useEffect(
    () => () => {
      if (dividerRafRef.current !== null) {
        cancelAnimationFrame(dividerRafRef.current);
      }
    },
    [],
  );

  const onDividerPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.pointerType === "mouse" && e.button !== 0) return;
    e.preventDefault();
    draggingRef.current = true;
    e.currentTarget.setPointerCapture(e.pointerId);
  };
  const onDividerPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!draggingRef.current || !twoPaneRef.current) return;
    const rect = twoPaneRef.current.getBoundingClientRect();
    const clientX = e.clientX;
    if (dividerRafRef.current !== null) return;
    dividerRafRef.current = requestAnimationFrame(() => {
      dividerRafRef.current = null;
      const next = Math.min(MAX_RIGHT_PANE, Math.max(MIN_RIGHT_PANE, rect.right - clientX));
      setRightWidth(next);
    });
  };
  const onDividerPointerUp = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!draggingRef.current) return;
    draggingRef.current = false;
    e.currentTarget.releasePointerCapture(e.pointerId);
    localStorage.setItem(RIGHT_PANE_KEY, String(rightWidth));
  };
  const onDividerKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const step = 16;
    let next: number;
    if (e.key === "ArrowLeft") {
      next = Math.min(MAX_RIGHT_PANE, rightWidth + step);
    } else if (e.key === "ArrowRight") {
      next = Math.max(MIN_RIGHT_PANE, rightWidth - step);
    } else if (e.key === "Home") {
      next = MIN_RIGHT_PANE;
    } else if (e.key === "End") {
      next = MAX_RIGHT_PANE;
    } else {
      return;
    }
    setRightWidth(next);
    localStorage.setItem(RIGHT_PANE_KEY, String(next));
    e.preventDefault();
  };

  const [previewNumPages, setPreviewNumPages] = useState(0);
  const [previewPage, setPreviewPage] = useState(1);
  const [containerWidth, setContainerWidth] = useState(0);
  const [isOnline, setIsOnline] = useState(navigator.onLine);
  const [showPdfPreview, setShowPdfPreview] = useState(false);
  const [pdfPreviewLoaded, setPdfPreviewLoaded] = useState(false);
  const pdfPreviewDocRef = useRef<PDFDocumentProxy | null>(null);
  const linkPdfInputRef = useRef<HTMLInputElement>(null);
  const previewScrollRef = useRef<HTMLDivElement | null>(null);
  const _pdfObsRef = useRef<ResizeObserver | null>(null);
  const pdfContainerRef = useCallback((el: HTMLDivElement | null) => {
    if (_pdfObsRef.current) {
      _pdfObsRef.current.disconnect();
      _pdfObsRef.current = null;
    }
    previewScrollRef.current = el;
    if (!el) return;
    const obs = new ResizeObserver((entries) => {
      setContainerWidth(entries[0].contentRect.width);
    });
    obs.observe(el);
    _pdfObsRef.current = obs;
  }, []);

  const {
    data: paper,
    isLoading: paperLoading,
    isFetching: paperFetching,
    error: paperError,
  } = useQuery({
    queryKey: ["paper", "sfk", sfk, selectedVersion ?? LATEST_VERSION_KEY],
    queryFn: () => getPaperBySfk(Number(sfk), selectedVersion ?? undefined),
    enabled: !!sfk && Number.isFinite(Number(sfk)),
    placeholderData: keepPreviousData,
  });

  const { data: versionsData } = useQuery({
    queryKey: ["paper", "versions", sfk],
    queryFn: () => getPaperVersions(Number(sfk)),
    enabled: !!sfk && Number.isFinite(Number(sfk)),
    placeholderData: keepPreviousData,
  });

  const versions = versionsData?.versions ?? [];

  // Other paper roots sharing this one's DOI — same work, different source
  // (e.g. arXiv vs OpenAlex/Crossref). Each candidate carries a confirm-guarded
  // "Merge into this paper" action (POST sfk/{fk}/merge).
  const { data: doiCandidates } = useQuery({
    queryKey: ["paper", "doi-candidates", sfk],
    queryFn: () => getDoiVersionCandidates(Number(sfk)),
    // Gated on a DOI actually being present — skips the round trip for the
    // (majority) of papers that have none.
    enabled: !!sfk && Number.isFinite(Number(sfk)) && !!paper?.doi,
  });

  // all_projects=true so project-scoped notes are visible alongside global
  // ones; each note carries its own scope, shown as a badge on the card.
  const { data: notesData, isLoading: notesLoading } = useQuery({
    queryKey: ["notes", paper?.source_id, { allProjects: true }],
    queryFn: () => getNotes(paper!.source_id, undefined, true),
    enabled: !!paper?.source_id,
  });

  // Highlights created in the PDF reader; shown here as a list with comments.
  // Same query key (allProjects) the reader uses, so the two share one cache.
  const { data: annotationsData, isLoading: annotationsLoading } = useQuery({
    queryKey: ["annotations", paper?.source_id, { allProjects: true }],
    queryFn: () => getAnnotations(paper!.source_id, undefined, true),
    enabled: !!paper?.source_id,
  });

  const { data: projectsData, isLoading: projectsLoading } = useQuery({
    queryKey: ["projects"],
    queryFn: () => listProjects(),
  });

  const isViewingLatest =
    selectedVersion === null || selectedVersion === versionsData?.latest_version;

  function handlePreviewPdf() {
    setShowPdfPreview(true);
  }

  const savePdfMutation = useMutation({
    mutationFn: async (sourceId: string) => {
      if (!pdfPreviewDocRef.current) throw new Error("PDF not loaded");
      const bytes = await pdfPreviewDocRef.current.getData();
      const path = `/api/papers/${encodeURIComponent(sourceId)}/pdf`;
      if (isTauri) {
        await libraryFetch(path, { method: "PUT", body: JSON.stringify({ file_b64: bytesToBase64(bytes) }) });
      } else {
        const form = new FormData();
        form.append("file", new Blob([bytes.slice()], { type: "application/pdf" }), `${sourceId}.pdf`);
        await libraryFetch(path, { method: "PUT", body: form });
      }
    },
    onSuccess: () => {
      invalidatePaperMutationQueries(queryClient);
    },
  });

  const linkPdfMutation = useMutation({
    mutationFn: async ({ sourceId, file }: { sourceId: string; file: File }) => {
      const path = `/api/papers/${encodeURIComponent(sourceId)}/pdf`;
      if (isTauri) {
        const file_b64 = bytesToBase64(new Uint8Array(await file.arrayBuffer()));
        await libraryFetch(path, { method: "PUT", body: JSON.stringify({ file_b64 }) });
      } else {
        const form = new FormData();
        form.append("file", file, file.name);
        await libraryFetch(path, { method: "PUT", body: form });
      }
    },
    onSuccess: () => {
      invalidatePaperMutationQueries(queryClient);
    },
  });

  // Pulls the paper's arXiv TeX source into papers_fts, so library search
  // matches the body and not just the metadata. On demand rather than on save:
  // arXiv paces requests ~7s apart and the tarballs run to megabytes.
  const indexFullTextMutation = useMutation({
    mutationFn: (force: boolean) => fetchFullText(paper!.source_id, force),
    onSuccess: () => {
      // Covers ["papers","search",…] too — indexing changes library FTS results.
      invalidatePaperMutationQueries(queryClient);
    },
  });

  const deleteNoteMutation = useMutation({
    mutationFn: (noteId: number) => deleteNote(noteId),
    onSuccess: () => {
      invalidateNoteQueries(queryClient);
    },
  });

  const [pendingDeleteId, setPendingDeleteId] = useState<number | null>(null);
  const deleteAnnotationMutation = useMutation({
    mutationFn: (id: number) => deleteAnnotation(id),
    onSuccess: () => {
      invalidateAnnotationQueries(queryClient);
    },
    onSettled: () => setPendingDeleteId(null),
  });

  // Merge a same-DOI duplicate INTO this paper (this paper's metadata stays
  // canonical; the duplicate's notes/memberships/tags/PDFs move over, then it
  // is deleted). Fans out like a hard delete: the duplicate vanishes from every
  // library/project/graph listing.
  // Two-click confirm (window.confirm is suppressed on Linux WebKitGTK — see
  // EditorPage's in-app dialog note): first click arms the row, second fires.
  const [armedMergeSfk, setArmedMergeSfk] = useState<number | null>(null);
  const mergeMutation = useMutation({
    mutationFn: (loserSfk: number) => mergePapers(Number(sfk), loserSfk),
    onSuccess: () => {
      // Reading statuses move with the merge inside the backend transaction
      // (winner wins); invalidatePaperQueries covers the "reading-status" key.
      invalidatePaperQueries(queryClient);
      // Covers "saved-pdfs" — a merge renames/deletes/adopts PDF files.
      invalidatePaperMutationQueries(queryClient);
      invalidateNoteQueries(queryClient);
      invalidateAnnotationQueries(queryClient);
    },
    onSettled: () => setArmedMergeSfk(null),
  });

  useEffect(() => {
    const handleOnline = () => setIsOnline(true);
    const handleOffline = () => setIsOnline(false);
    window.addEventListener("online", handleOnline);
    window.addEventListener("offline", handleOffline);
    return () => {
      window.removeEventListener("online", handleOnline);
      window.removeEventListener("offline", handleOffline);
    };
  }, []);

  const { reset: resetSavePdf } = savePdfMutation;
  const { reset: resetLinkPdf } = linkPdfMutation;
  const { reset: resetIndexFullText } = indexFullTextMutation;

  useEffect(() => {
    setPreviewNumPages(0);
    setPreviewPage(1);
    setShowPdfPreview(false);
    setPdfPreviewLoaded(false);
    setOpenNativeError(null);
    setOpenNativeLoading(false);
    pdfPreviewDocRef.current = null;
    resetSavePdf();
    resetLinkPdf();
    resetIndexFullText();
    return () => {
      openNativeAbortRef.current?.abort();
      openNativeAbortRef.current = null;
    };
  }, [
    sfk,
    selectedVersion,
    paper?.has_pdf,
    resetSavePdf,
    resetLinkPdf,
    resetIndexFullText,
  ]);

  function handleNotesSaved() {
    invalidateNoteQueries(queryClient);
    setShowAddNote(false);
    setEditingNoteId(null);
    deleteNoteMutation.reset();
  }

  function handleDeleteNote(note: Note) {
    if (!deleteNoteMutation.isPending) {
      deleteNoteMutation.mutate(note.id);
    }
  }

  function handlePaperSaved(_updated: Paper) {
    invalidatePaperQueries(queryClient);
  }

  async function handleOpenNative() {
    if (!paper?.has_pdf || openNativeLoading) return;
    setOpenNativeError(null);
    setOpenNativeLoading(true);
    const controller = new AbortController();
    openNativeAbortRef.current = controller;
    try {
      // Remote backend: pdf-path is denied by the node (403) — fetch the bytes
      // over the byte lane into the local cache and open that path instead.
      const remote = useBackendStore.getState().defaultBackend;
      const version = paper.version > 0 ? paper.version : undefined;
      const path = remote
        ? await remotePdfPath(remote.id, paper.source_id, version)
        : (
            await apiFetch<{ path: string }>(
              `/api/papers/${encodeURIComponent(paper.source_id)}/pdf-path${version !== undefined ? `?version=${version}` : ""}`,
              { signal: controller.signal }
            )
          ).path;
      if (controller.signal.aborted) return;
      if (typeof path !== "string" || !path) throw new Error("Invalid response from pdf-path endpoint");
      await openPdfInSystem(path);
    } catch (err) {
      if (controller.signal.aborted) return;
      setOpenNativeError(
        typeof err === "string"
          ? err
          : errText(err, "Failed to open PDF")
      );
    } finally {
      if (!controller.signal.aborted) setOpenNativeLoading(false);
    }
  }

  if (paperLoading) {
    return (
      <div className="flex items-center justify-center h-full">
        <Spinner size={28} />
      </div>
    );
  }

  if (paperError || !paper) {
    return (
      <div className="flex items-center justify-center h-full">
        <p className="text-sm" style={{ color: "var(--color-danger)" }}>
          {errText(paperError, "Paper not found.")}
        </p>
      </div>
    );
  }

  const authors = paper.authors;
  const notes = notesData?.notes ?? [];
  const annotations = annotationsData?.annotations ?? [];
  const editingNote =
    editingNoteId != null ? notes.find((n) => n.id === editingNoteId) ?? null : null;
  const tags = paper.tags ?? [];

  // Projects this paper belongs to populate the note scope picker.
  const paperProjects = (projectsData?.projects ?? []).filter((p) =>
    p.source_ids.includes(paper.source_id),
  );
  const fromProjectId =
    (location.state as { fromProjectId?: number } | null)?.fromProjectId ?? null;
  const defaultProjectId =
    fromProjectId != null && paperProjects.some((p) => p.id === fromProjectId)
      ? fromProjectId
      : null;
  const versionedList = versions
    .filter((v) => v.version >= 1)
    .sort((a, b) => a.version - b.version);

  const hasPdfContent = paper.has_pdf || showPdfPreview;

  // Mirrors the backend guard (service::paper::source_fetch_url): only arXiv
  // publishes a TeX tarball, and only a /pdf/ URL can be rewritten to /src/.
  const canIndexFullText =
    isViewingLatest && paper.source === "arxiv" && !!paper.url?.includes("/pdf/");
  const indexResult = indexFullTextMutation.data;

  const fadeStyle = {
    opacity: paperFetching && !paperLoading ? 0.6 : 1,
    transition: "opacity 0.15s",
  };

  function goToPreviewPage(target: number) {
    if (previewNumPages <= 0) return;
    const next = Math.min(Math.max(target, 1), previewNumPages);
    setPreviewPage(next);
    const scroller = previewScrollRef.current;
    const pageEl = scroller?.querySelectorAll<HTMLElement>(".react-pdf__Page")[next - 1];
    if (scroller && pageEl) scroller.scrollTop = pageEl.offsetTop;
  }

  return (
    <div className="h-full overflow-hidden flex flex-col">
      {/* Header strip: back · title · source id · edit */}
      <div
        className="flex items-center gap-4 px-6 py-3 border-b border-border shrink-0"
        style={fadeStyle}
      >
        <Button variant="ghost" size="sm" onClick={() => navigate(-1)}>
          ← Library
        </Button>
        <h1 className="font-display text-text text-[17px] leading-tight truncate flex-1 min-w-0">
          <MathText forceInline>{paper.title}</MathText>
        </h1>
        <span className="font-mono text-xs text-ink3 shrink-0">
          {labelForSource(paper)}
        </span>
        {paper.has_pdf && isTauri && (
          <Button variant="muted" size="sm" onClick={handleOpenNative} disabled={openNativeLoading}>
            {openNativeLoading ? "Opening…" : "Open in system viewer"}
          </Button>
        )}
        {openNativeError && (
          <span className="text-xs text-danger shrink-0">{openNativeError}</span>
        )}
        {canIndexFullText && (
          <Button
            variant="muted"
            size="sm"
            onClick={() => indexFullTextMutation.mutate(paper.downloaded_source)}
            disabled={indexFullTextMutation.isPending || !isOnline}
            title="Download the arXiv TeX source so library search can match the paper's body"
          >
            {indexFullTextMutation.isPending
              ? "Indexing…"
              : paper.downloaded_source
                ? "Re-index full text"
                : "Index full text"}
          </Button>
        )}
        {canIndexFullText && !isOnline && <span className="text-xs text-muted">Offline</span>}
        {canIndexFullText && indexFullTextMutation.error && (
          <span className="text-xs text-danger shrink-0">
            {errText(indexFullTextMutation.error, "Failed to index full text")}
          </span>
        )}
        {canIndexFullText && indexResult && !indexFullTextMutation.isPending && (
          <span className="text-xs text-ink3 shrink-0">
            {indexResult.indexed
              ? (indexResult.chars ?? 0) === 0
                ? "No TeX source found; marked as checked"
                : `Indexed ${(indexResult.chars ?? 0).toLocaleString()} characters`
              : indexResult.reason}
          </span>
        )}
        {isViewingLatest && (
          <Button variant="muted" size="sm" onClick={() => setShowEditor(true)}>
            Edit
          </Button>
        )}
      </div>

      {/* Two-pane row: each pane scrolls independently */}
      <div
        ref={twoPaneRef}
        className={`flex-1 min-h-0 overflow-hidden ${hasPdfContent ? "grid grid-rows-[1fr_1fr] grid-cols-1 lg:grid-rows-1 lg:grid-cols-[1fr_388px]" : "flex flex-col"}`}
        style={
          hasPdfContent && isWide
            ? { gridTemplateColumns: `minmax(0,1fr) 6px ${rightWidth}px` }
            : undefined
        }
      >
        {/* Left pane: PDF. While the metadata editor (a non-modal dialog with
            a z-40 click shield) is open, raise this pane above the shield so
            the PDF stays selectable while the rest of the page is inert.
            Only when it actually shows a PDF — the no-PDF strip holds
            mutating save/link buttons that must stay behind the shield. */}
        <div className={`${showEditor && hasPdfContent ? "relative z-[45] " : ""}${hasPdfContent ? "min-h-0 overflow-y-auto bg-surface2 border-r border-border" : "shrink-0 flex items-center gap-3 flex-wrap px-6 py-3 bg-surface2 border-b border-border"}`}>
          <div className={hasPdfContent ? "h-full flex flex-col" : "contents"}>
            <PdfPane
              paper={paper}
              isViewingLatest={isViewingLatest}
              isOnline={isOnline}
              projectId={defaultProjectId}
              onPreview={handlePreviewPdf}
              savePdfMutation={savePdfMutation}
              linkPdfMutation={linkPdfMutation}
              linkPdfInputRef={linkPdfInputRef}
              showPdfPreview={showPdfPreview}
              previewNumPages={previewNumPages}
              setPreviewNumPages={setPreviewNumPages}
              previewPage={previewPage}
              setPreviewPage={setPreviewPage}
              goToPreviewPage={goToPreviewPage}
              pdfPreviewLoaded={pdfPreviewLoaded}
              setPdfPreviewLoaded={setPdfPreviewLoaded}
              pdfPreviewDocRef={pdfPreviewDocRef}
              pdfContainerRef={pdfContainerRef}
              containerWidth={containerWidth}
              asStrip={!hasPdfContent}
            />
          </div>
        </div>

        {/* Draggable divider (side-by-side layout only) */}
        {hasPdfContent && isWide && (
          <div
            role="separator"
            aria-orientation="vertical"
            aria-label="Resize panels"
            aria-valuenow={rightWidth}
            aria-valuemin={MIN_RIGHT_PANE}
            aria-valuemax={MAX_RIGHT_PANE}
            tabIndex={0}
            onPointerDown={onDividerPointerDown}
            onPointerMove={onDividerPointerMove}
            onPointerUp={onDividerPointerUp}
            onPointerCancel={onDividerPointerUp}
            onKeyDown={onDividerKeyDown}
            className="h-full cursor-col-resize bg-border hover:bg-accent transition-colors"
            style={{ touchAction: "none" }}
          />
        )}

        {/* Right pane: identity + Details/Notes */}
        <div className={`overflow-y-auto bg-panel ${hasPdfContent ? "min-h-0" : "flex-1 min-h-0"}`}>
          <div className={hasPdfContent ? "px-[18px] py-5 space-y-5" : "max-w-[760px] mx-auto px-8 py-6 space-y-5"}>
            {/* Identity block */}
            <div className="space-y-3" style={fadeStyle}>
              <h2 className="font-display text-text text-[21px] leading-snug">
                <MathText forceInline>{paper.title}</MathText>
              </h2>

              {authors.length > 0 && (
                <div className="space-y-1.5">
                  <MonoLabel>Authors</MonoLabel>
                  <p className="text-muted text-sm">{authors.join(", ")}</p>
                </div>
              )}

              {/* Meta row */}
              <div className="flex flex-wrap items-center gap-3 text-sm">
                {paper.published && (
                  <span className="text-muted">{formatDate(paper.published)}</span>
                )}
                {paper.doi && (
                  <>
                    <span className="text-border">·</span>
                    <a
                      href={`https://doi.org/${paper.doi}`}
                      target="_blank"
                      rel="noopener noreferrer"
                      className="transition-colors hover:text-text"
                      style={{ color: "var(--color-accent)" }}
                    >
                      DOI: {paper.doi}
                    </a>
                  </>
                )}
                {paper.category && (
                  <Badge
                    style={{
                      borderColor: "var(--color-accent)",
                      color: "var(--color-accent)",
                      backgroundColor:
                        "color-mix(in srgb, var(--color-accent) 12%, transparent)",
                    }}
                  >
                    {paper.category}
                  </Badge>
                )}
                {versionedList.length > 1 ? (
                  <select
                    value={selectedVersion ?? versionsData?.latest_version}
                    onChange={(e) => {
                      const v = Number(e.target.value);
                      setSelectedVersion(
                        v === versionsData?.latest_version ? null : v
                      );
                    }}
                    className="inline-flex items-center rounded-full font-medium border border-[var(--color-border)] bg-[var(--color-panel)] text-[var(--color-text)] px-2 py-0.5 text-xs cursor-pointer"
                    aria-label="Select version"
                  >
                    {versionedList.map((v) => {
                      const dateStr = v.updated ?? v.published;
                      const label = dateStr ? ` · ${formatDate(dateStr)}` : "";
                      const isLatest = v.version === versionsData?.latest_version;
                      return (
                        <option key={v.version} value={v.version}>
                          v{v.version}
                          {isLatest ? " (latest)" : ""}
                          {label}
                        </option>
                      );
                    })}
                  </select>
                ) : (
                  paper.version > 0 && <Badge>v{paper.version}</Badge>
                )}
              </div>

              {/* Tags */}
              {tags.length > 0 && (
                <div className="space-y-1.5">
                  <MonoLabel>Tags</MonoLabel>
                  <div className="flex flex-wrap gap-1.5">
                    {tags.map((tag) => (
                      <TagBadge key={tag} label={tag} />
                    ))}
                  </div>
                </div>
              )}

              {/* Same DOI, different source — likely duplicates, mergeable into this paper */}
              {doiCandidates && doiCandidates.length > 0 && (
                <div
                  className="rounded-md border px-3 py-2 text-sm space-y-1.5"
                  style={{ borderColor: "var(--color-border)", backgroundColor: "var(--color-panel)" }}
                >
                  <p style={{ color: "var(--color-text)" }}>
                    Same DOI found in {doiCandidates.length} other record
                    {doiCandidates.length > 1 ? "s" : ""}. These are likely to be the same paper.
                  </p>
                  <div className="space-y-1">
                    {doiCandidates.map((c) => (
                      <div key={c.source_fk} className="flex items-center gap-2">
                        <button
                          type="button"
                          className="text-xs underline underline-offset-2 hover:opacity-80 text-left"
                          style={{ color: "var(--color-accent)" }}
                          onClick={() => navigate(`/library/${c.source_fk}`)}
                        >
                          {c.source ?? c.source_id}: <MathText forceInline>{c.title}</MathText>
                        </button>
                        {/* Two-click confirm: arm, then fire. The merge deletes the duplicate record. */}
                        <button
                          type="button"
                          className="text-xs rounded border px-1.5 py-0.5 hover:opacity-80 shrink-0"
                          style={{
                            borderColor:
                              armedMergeSfk === c.source_fk
                                ? "var(--color-danger)"
                                : "var(--color-border)",
                            color:
                              armedMergeSfk === c.source_fk
                                ? "var(--color-danger)"
                                : "var(--color-text-muted)",
                          }}
                          disabled={mergeMutation.isPending}
                          onClick={() => {
                            if (armedMergeSfk === c.source_fk) {
                              mergeMutation.mutate(c.source_fk);
                            } else {
                              setArmedMergeSfk(c.source_fk);
                            }
                          }}
                        >
                          {mergeMutation.isPending && mergeMutation.variables === c.source_fk
                            ? "Merging…"
                            : armedMergeSfk === c.source_fk
                              ? "Confirm — deletes the duplicate"
                              : "Merge into this paper"}
                        </button>
                      </div>
                    ))}
                  </div>
                  {mergeMutation.isError && (
                    <p className="text-xs" style={{ color: "var(--color-danger)" }}>
                      {(mergeMutation.error as Error).message}
                    </p>
                  )}
                </div>
              )}
            </div>

            {/* Tabs: Details | Notes */}
            <Tabs defaultValue="details">
              <TabsList>
                <TabsTrigger value="details">Details</TabsTrigger>
                <TabsTrigger value="notes">
                  Notes{notes.length > 0 ? ` (${notes.length})` : ""}
                </TabsTrigger>
                <TabsTrigger value="annotations">
                  Annotations{annotations.length > 0 ? ` (${annotations.length})` : ""}
                </TabsTrigger>
              </TabsList>

              {/* Details tab: abstract */}
              <TabsContent value="details" className="pt-5">
                {paper.summary ? (
                  <div className="space-y-2">
                    <MonoLabel as="h3">Abstract</MonoLabel>
                    <div className="text-muted text-sm leading-relaxed whitespace-pre-wrap">
                      <MathText forceInline>{paper.summary}</MathText>
                    </div>
                  </div>
                ) : (
                  <p className="text-muted text-sm">No abstract available.</p>
                )}
              </TabsContent>

              <TabsContent value="notes" forceMount className="pt-5 space-y-4 data-[state=inactive]:hidden">
                <div className="flex items-center justify-between">
                  <MonoLabel as="h3">Notes</MonoLabel>
                  {!showAddNote && !editingNote && (
                    <Button
                      variant="muted"
                      size="sm"
                      onClick={() => {
                        deleteNoteMutation.reset();
                        setShowAddNote(true);
                      }}
                    >
                      + Add note
                    </Button>
                  )}
                </div>

                {showAddNote && !editingNote && (
                  <Card>
                    <NoteEditor
                      sourceId={paper.source_id}
                      projects={paperProjects}
                      projectsLoading={projectsLoading}
                      defaultProjectId={defaultProjectId}
                      onSave={handleNotesSaved}
                      onCancel={() => setShowAddNote(false)}
                    />
                  </Card>
                )}

                {editingNote && (
                  <Card>
                    <NoteEditor
                      key={editingNote.id}
                      sourceId={paper.source_id}
                      projects={paperProjects}
                      initialNote={editingNote}
                      onSave={handleNotesSaved}
                      onCancel={() => setEditingNoteId(null)}
                    />
                  </Card>
                )}

                {deleteNoteMutation.isError && (
                  <p
                    className="text-sm text-center"
                    style={{ color: "var(--color-danger)" }}
                  >
                    {errText(deleteNoteMutation.error, "Failed to delete the note.")}
                  </p>
                )}

                {notesLoading ? (
                  <div className="flex justify-center py-6">
                    <Spinner size={20} />
                  </div>
                ) : (
                  <>
                    {notes.length === 0 && !showAddNote && !editingNote && (
                      <p className="text-muted text-sm text-center py-8">
                        No notes yet. Add one above.
                      </p>
                    )}
                    {notes.length > 0 && (
                      <div className="space-y-3">
                        {notes.map((note) => (
                          <NoteCard
                            key={note.id}
                            note={note}
                            projects={paperProjects}
                            onEdit={(n) => {
                              deleteNoteMutation.reset();
                              setEditingNoteId(n.id);
                              setShowAddNote(false);
                            }}
                            onDelete={handleDeleteNote}
                          />
                        ))}
                      </div>
                    )}
                  </>
                )}
              </TabsContent>

              <TabsContent value="annotations" className="pt-5 space-y-4">
                <MonoLabel as="h3">Annotations</MonoLabel>
                {annotationsLoading ? (
                  <div className="flex justify-center py-6">
                    <Spinner size={20} />
                  </div>
                ) : annotations.length === 0 ? (
                  <p className="text-muted text-sm text-center py-8">
                    No annotations yet. Highlight text in the saved PDF to create one.
                  </p>
                ) : (
                  <div className="space-y-3">
                    {annotations.map((a) => (
                      <AnnotationCard
                        key={a.id}
                        annotation={a}
                        isPending={
                          a.id === pendingDeleteId &&
                          deleteAnnotationMutation.isPending
                        }
                        onDelete={() => {
                          if (!deleteAnnotationMutation.isPending) {
                            setPendingDeleteId(a.id);
                            deleteAnnotationMutation.mutate(a.id);
                          }
                        }}
                      />
                    ))}
                  </div>
                )}
                {deleteAnnotationMutation.isError && (
                  <p
                    className="text-sm text-center"
                    style={{ color: "var(--color-danger)" }}
                  >
                    Failed to delete the annotation.
                  </p>
                )}
              </TabsContent>
            </Tabs>
          </div>
        </div>
      </div>

      {showEditor && (
        <PaperMetadataEditor
          onClose={() => setShowEditor(false)}
          paper={paper}
          onSaved={handlePaperSaved}
        />
      )}
    </div>
  );
}

// One annotation in the Annotations tab: a color chip + quoted highlight + the
// written comment. The quote expands to full text, and the comment is editable
// inline (add/edit/remove) so a highlight can carry a comment without opening the
// reader popup. All edits invalidate the shared cache, keeping the reader in sync.
function AnnotationCard({
  annotation,
  onDelete,
  isPending,
}: {
  annotation: Annotation;
  onDelete: () => void;
  isPending: boolean;
}) {
  const queryClient = useQueryClient();
  const anchor = parseAnchor(annotation.anchor);
  const [expanded, setExpanded] = useState(false);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(annotation.comment);
  // Comment as seen when editing opened; diverges if another surface saves first.
  const [baseComment, setBaseComment] = useState(annotation.comment);
  const stale = baseComment !== annotation.comment;

  const updateMutation = useMutation({
    mutationFn: (comment: string) => updateAnnotation(annotation.id, comment),
    onSuccess: (_data, comment) => {
      setBaseComment(comment);
      invalidateAnnotationQueries(queryClient);
      setEditing(false);
    },
  });

  // Long quotes clamp to 3 lines; offer the toggle only when there's plausibly
  // more to show (length heuristic, not exact overflow measurement).
  const quote = anchor?.quote ?? "";
  const clampable = quote.length > 160;

  return (
    <Card variant="translucent">
      <div className="flex items-start gap-2.5">
        <span
          className="mt-1 h-3 w-3 shrink-0 rounded-full border border-black/20"
          style={{ backgroundColor: anchor?.color ?? "var(--color-muted)" }}
          aria-hidden
        />
        <div className="min-w-0 flex-1 space-y-1.5">
          {quote && (
            <p
              className={`text-sm text-text italic ${
                clampable && !expanded ? "line-clamp-3" : "whitespace-pre-wrap"
              }`}
            >
              “{quote}”
            </p>
          )}
          {clampable && (
            <button
              onClick={() => setExpanded((v) => !v)}
              className="text-xs font-medium text-accent hover:underline"
            >
              {expanded ? "Show less" : "Show more"}
            </button>
          )}

          {editing ? (
            <div className="space-y-1.5">
              <textarea
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={submitOnCtrlEnter(() => {
                  if (!(updateMutation.isPending || draft === annotation.comment || stale))
                    updateMutation.mutate(draft);
                })}
                placeholder="Add a comment…"
                rows={3}
                autoFocus
                className="w-full resize-none rounded border border-border bg-surface2 px-2 py-1.5 text-sm text-text focus:outline-none focus:border-accent"
              />
              <div className="flex items-center gap-3 text-xs">
                <button
                  disabled={updateMutation.isPending || draft === annotation.comment || stale}
                  onClick={() => updateMutation.mutate(draft)}
                  className="font-medium text-accent hover:underline disabled:opacity-40"
                >
                  {updateMutation.isPending ? "Saving…" : "Save"}
                </button>
                <button
                  onClick={() => {
                    setDraft(annotation.comment);
                    updateMutation.reset();
                    setEditing(false);
                  }}
                  className="font-medium text-muted hover:underline"
                >
                  Cancel
                </button>
                {stale ? (
                  <span style={{ color: "var(--color-danger)" }}>
                    Comment was updated elsewhere. Cancel to reload before saving.
                  </span>
                ) : (
                  updateMutation.isError && (
                    <span style={{ color: "var(--color-danger)" }}>
                      Couldn't save. Try again.
                    </span>
                  )
                )}
              </div>
            </div>
          ) : (
            annotation.comment && (
              <p className="text-sm text-muted whitespace-pre-wrap">
                {annotation.comment}
              </p>
            )
          )}

          <div className="flex items-center gap-3 text-xs text-ink3">
            {anchor && <span>p. {anchor.page}</span>}
            {!editing && (
              <button
                onClick={() => {
                  setDraft(annotation.comment);
                  setBaseComment(annotation.comment);
                  updateMutation.reset();
                  setEditing(true);
                }}
                className="font-medium text-accent hover:underline"
              >
                {annotation.comment ? "Edit comment" : "Add comment"}
              </button>
            )}
            <button
              onClick={onDelete}
              disabled={isPending || updateMutation.isPending}
              className="ml-auto font-medium text-[var(--color-danger)] hover:underline disabled:opacity-50"
            >
              {isPending ? "Deleting…" : "Delete"}
            </button>
          </div>
        </div>
      </div>
    </Card>
  );
}

type Mutation<TVars> = {
  mutate: (vars: TVars) => void;
  isPending: boolean;
  isError: boolean;
  isSuccess: boolean;
  error: unknown;
};

interface PdfPaneProps {
  paper: Paper;
  isViewingLatest: boolean;
  isOnline: boolean;
  asStrip?: boolean;
  onPreview: () => void;
  savePdfMutation: Mutation<string>;
  linkPdfMutation: Mutation<{ sourceId: string; file: File }>;
  linkPdfInputRef: React.RefObject<HTMLInputElement | null>;
  showPdfPreview: boolean;
  previewNumPages: number;
  setPreviewNumPages: (n: number) => void;
  previewPage: number;
  setPreviewPage: (n: number) => void;
  goToPreviewPage: (n: number) => void;
  pdfPreviewLoaded: boolean;
  setPdfPreviewLoaded: (v: boolean) => void;
  pdfPreviewDocRef: React.MutableRefObject<PDFDocumentProxy | null>;
  pdfContainerRef: (el: HTMLDivElement | null) => void;
  containerWidth: number;
  projectId?: number | null;
}

function PdfPane({
  paper,
  isViewingLatest,
  isOnline,
  onPreview,
  savePdfMutation,
  linkPdfMutation,
  linkPdfInputRef,
  showPdfPreview,
  previewNumPages,
  setPreviewNumPages,
  previewPage,
  setPreviewPage,
  goToPreviewPage,
  pdfPreviewLoaded,
  setPdfPreviewLoaded,
  pdfPreviewDocRef,
  pdfContainerRef,
  containerWidth,
  projectId,
  asStrip,
}: PdfPaneProps) {
  const previewScrollRafRef = useRef<number | null>(null);
  useEffect(
    () => () => {
      if (previewScrollRafRef.current !== null) {
        cancelAnimationFrame(previewScrollRafRef.current);
      }
    },
    [],
  );

  // Remote backend: the linxiv:// scheme only serves the LOCAL library, so pull
  // the bytes through `remote_pdf` into the local cache first, then hand the
  // cached file to the same reader via convertFileSrc (the assetProtocol scope
  // covers remote_pdf_cache).
  const remoteBackend = useBackendStore((s) => s.defaultBackend);
  const remotePdfQ = useQuery({
    queryKey: ["remote-pdf", remoteBackend?.id, paper.source_id, paper.version],
    enabled: !!remoteBackend && paper.has_pdf,
    staleTime: Infinity,
    queryFn: async () => {
      const path = await remotePdfPath(
        remoteBackend!.id,
        paper.source_id,
        paper.version > 0 ? paper.version : undefined,
      );
      const { convertFileSrc } = await import("@tauri-apps/api/core");
      return convertFileSrc(path);
    },
  });

  if (paper.has_pdf) {
    // Remote: loading/error surface while the byte-lane fetch fills the cache.
    if (remoteBackend && remotePdfQ.data === undefined) {
      return (
        <div className="flex h-full w-full items-center justify-center bg-panel px-6">
          {remotePdfQ.isError ? (
            <p className="text-sm text-center" style={{ color: "var(--color-danger)" }}>
              {errText(remotePdfQ.error, "Failed to fetch the PDF from the remote backend")}
            </p>
          ) : (
            <div className="flex items-center gap-2 text-sm text-muted">
              <Spinner size={16} />
              Fetching PDF from {remoteBackend.label}…
            </div>
          )}
        </div>
      );
    }
    // Saved PDF: the annotating reader (select→highlight, click→comment/delete)
    // replaces the plain iframe so highlights can overlay the pages.
    return (
      <div className="relative w-full h-full min-h-0 flex flex-col">
        <div className="flex-1 min-h-0 w-full overflow-hidden bg-panel">
          <PdfReader
            file={
              remoteBackend
                ? remotePdfQ.data!
                : getPaperPdfUrl(
                    paper.source_id,
                    paper.version > 0 ? paper.version : undefined
                  )
            }
            sourceId={paper.source_id}
            version={paper.version}
            projectId={projectId}
            errorUrl={paper.url}
          />
        </div>
      </div>
    );
  }

  // arXiv latest, no saved PDF: preview via react-pdf with save-to-library.
  if (paper.source === "arxiv" && isViewingLatest) {
    return (
      <div className={asStrip ? "flex items-center gap-3 flex-wrap" : "relative w-full h-full min-h-0 flex flex-col gap-3"}>
        {!showPdfPreview && (
          <div className="flex items-center gap-3 flex-wrap self-center">
            <Button
              variant="muted"
              onClick={onPreview}
              disabled={!isOnline}
            >
              Preview PDF
            </Button>
            {!isOnline && <span className="text-xs text-muted">Offline</span>}
          </div>
        )}
        {!showPdfPreview && paper.url && (
          <a
            href={paper.url}
            target="_blank"
            rel="noopener noreferrer"
            className="text-sm hover:underline self-center"
            style={{ color: "var(--color-accent)" }}
          >
            View online ↗
          </a>
        )}
        {showPdfPreview &&
          (paper.url ? (
            <>
              <div className="absolute top-3 left-1/2 -translate-x-1/2 z-10 inline-flex items-center gap-3 flex-wrap rounded-full bg-panel border border-border shadow-card px-4 py-2">
                <Button
                  variant="primary"
                  size="sm"
                  disabled={savePdfMutation.isPending || savePdfMutation.isSuccess || !pdfPreviewLoaded}
                  onClick={() => savePdfMutation.mutate(paper.source_id)}
                >
                  {savePdfMutation.isPending ? (
                    <span className="flex items-center gap-1.5">
                      <Spinner size={12} /> Saving…
                    </span>
                  ) : savePdfMutation.isSuccess ? (
                    "Saved!"
                  ) : (
                    "Save PDF to library"
                  )}
                </Button>
                {savePdfMutation.isError && (
                  <span className="text-xs" style={{ color: "var(--color-danger)" }}>
                    {errText(savePdfMutation.error, "Save failed")}
                  </span>
                )}
              </div>
              <div className="relative w-full flex-1 min-h-0">
                <div
                  ref={pdfContainerRef}
                  onScroll={(e) => {
                    const scroller = e.currentTarget;
                    if (previewScrollRafRef.current !== null) return;
                    previewScrollRafRef.current = requestAnimationFrame(() => {
                      previewScrollRafRef.current = null;
                      if (!scroller.isConnected) return;
                      const pages = scroller.querySelectorAll<HTMLElement>(".react-pdf__Page");
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
                      setPreviewPage(nearest);
                    });
                  }}
                  className="w-full h-full overflow-y-auto bg-[#525659]"
                >
                  <Document
                    file={getPdfProxyUrl(paper.url)}
                    onLoadSuccess={(pdf) => {
                      setPreviewNumPages(pdf.numPages);
                      pdfPreviewDocRef.current = pdf;
                      setPdfPreviewLoaded(true);
                    }}
                    loading={
                      <div className="flex items-center justify-center gap-2 py-16 text-white/60 text-sm">
                        <Spinner size={16} /> Loading PDF…
                      </div>
                    }
                    error={
                      <div className="flex flex-col items-center justify-center gap-3 py-16 text-sm">
                        <span className="text-danger">Failed to load PDF.</span>
                        <a
                          href={paper.url}
                          target="_blank"
                          rel="noreferrer"
                          className="text-accent hover:underline"
                        >
                          Open in browser
                        </a>
                      </div>
                    }
                  >
                    {Array.from({ length: previewNumPages }, (_, i) => (
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
                <PagePill page={previewPage} total={previewNumPages} onGo={goToPreviewPage} />
              </div>
            </>
          ) : (
            <p className="text-muted text-sm">No PDF URL available for preview.</p>
          ))}
      </div>
    );
  }

  // Linked / other version: open external or link a local file.
  return (
    <div className="w-full max-w-[560px] flex items-center gap-3 flex-wrap justify-center">
      {paper.url && (
        <a
          href={paper.url}
          target="_blank"
          rel="noopener noreferrer"
          className="inline-flex items-center justify-center gap-1.5 font-medium transition-colors px-3.5 py-1.5 text-sm rounded-md bg-[var(--color-panel)] text-[var(--color-text)] border border-[var(--color-border)] hover:border-[var(--color-accent)] hover:text-[var(--color-accent)]"
        >
          Open PDF ↗
        </a>
      )}
      <input
        ref={linkPdfInputRef}
        type="file"
        accept=".pdf,application/pdf"
        className="hidden"
        onChange={(e) => {
          const file = e.target.files?.[0];
          if (file) linkPdfMutation.mutate({ sourceId: paper.source_id, file });
          e.target.value = "";
        }}
      />
      <Button
        variant="muted"
        size="sm"
        disabled={linkPdfMutation.isPending}
        onClick={() => linkPdfInputRef.current?.click()}
      >
        {linkPdfMutation.isPending ? (
          <span className="flex items-center gap-1.5">
            <Spinner size={12} /> Linking…
          </span>
        ) : (
          "Link PDF"
        )}
      </Button>
      {linkPdfMutation.isError && (
        <span className="text-xs" style={{ color: "var(--color-danger)" }}>
          {errText(linkPdfMutation.error, "Link failed")}
        </span>
      )}
      {!paper.url && !linkPdfMutation.isPending && !linkPdfMutation.isError && (
        <span className="text-muted text-sm">No PDF available for this version.</span>
      )}
    </div>
  );
}
