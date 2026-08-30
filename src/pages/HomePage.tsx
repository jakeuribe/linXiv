import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router";
import { getSettings, getStats } from "../api/settings";
import { dismissFeedEntry, getFeed } from "../api/feed";
import { fetchArxiv } from "../api/search";
import { getPaperPdfUrl } from "../api/papers";
import { Spinner } from "../components/ui/spinner";
import { Button } from "../components/ui/button";
import { Dialog } from "../components/ui/dialog";
import { PaperList } from "../components/papers/PaperList";
import { Card, MonoLabel, SectionTitle } from "../components/ui/card";
import { DismissControls } from "../components/feed/DismissControls";
import type { FeedEntry, Paper, SearchResult } from "../types/api";
import { MathText } from "../lib/tex";
import { invalidatePaperMutationQueries } from "../lib/paperMutations";
import { errText } from "../lib/errText";

interface StatCardProps {
  label: string;
  value: number | string | undefined;
  hint?: string;
  to?: string;
}

function StatCard({ label, value, hint, to }: StatCardProps) {
  const navigate = useNavigate();
  const interactive = to !== undefined;

  const content = (
    <div className="flex flex-col gap-1 text-left">
      <span
        className="font-display text-[30px] leading-none"
        style={{ color: "var(--color-accent)" }}
      >
        {value ?? "—"}
      </span>
      <span className="text-[12.5px] text-muted">{label}</span>
      {hint !== undefined && <MonoLabel className="mt-1 normal-case">{hint}</MonoLabel>}
    </div>
  );

  if (interactive) {
    return (
      <button
        type="button"
        className="group block w-full rounded-[var(--card-radius)] text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--color-accent)]"
        onClick={() => navigate(to)}
      >
        <Card className="h-full cursor-pointer transition-colors group-hover:border-accent">
          {content}
        </Card>
      </button>
    );
  }
  return <Card>{content}</Card>;
}

const FEED_ENTRY_LIMIT = 25;

function FeedRow({
  entry,
  alreadySaved,
  onDismissed,
}: {
  entry: FeedEntry;
  alreadySaved?: boolean;
  onDismissed: () => void;
}) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [saveState, setSaveState] = useState<"idle" | "saving" | "saved" | "error">("idle");
  const [dismissing, setDismissing] = useState(false);
  const [dismissError, setDismissError] = useState(false);
  const [confirmPermanent, setConfirmPermanent] = useState(false);

  async function handleSave(arxivId: string) {
    setSaveState("saving");
    try {
      await fetchArxiv(arxivId, true);
      setSaveState("saved");
      invalidatePaperMutationQueries(queryClient);
    } catch (err) {
      console.error(err);
      setSaveState("error");
    }
  }

  // Same in-house preview machinery the search page uses (/pdf-preview): a
  // saved paper reads through our own PDF proxy, an unsaved one hits arXiv
  // directly (with a CORS-proxy fallback baked into PdfPreviewPage itself).
  function handlePreview() {
    if (entry.arxiv_id === null || entry.version === null) return;
    const paperUrl = alreadySaved
      ? getPaperPdfUrl(entry.arxiv_id, entry.version)
      : `https://arxiv.org/pdf/${entry.arxiv_id}v${entry.version}`;
    const result: SearchResult = {
      source_id: entry.arxiv_id,
      version: entry.version,
      title: entry.title,
      summary: entry.summary,
      authors: entry.authors,
      published: entry.published,
      paper_url: paperUrl,
      primary_category: "",
      entry_id: `arxiv:${entry.arxiv_id}`,
    };
    navigate("/pdf-preview", { state: { result, isSaved: alreadySaved ?? false } });
  }

  async function handleDismiss(arxivId: string, version: number, permanent = false) {
    setDismissing(true);
    setDismissError(false);
    try {
      await dismissFeedEntry(arxivId, version, permanent);
      onDismissed();
    } catch (err) {
      console.error(err);
      setDismissing(false);
      setDismissError(true);
    }
  }

  return (
    <div className="flex items-start justify-between gap-4 border-b border-border py-3 last:border-0">
      <div className="min-w-0">
        <a
          href={entry.link || undefined}
          target="_blank"
          rel="noopener noreferrer"
          className={`block text-sm font-medium text-text transition-colors${
            entry.link ? " hover:text-accent" : " cursor-default"
          }`}
        >
          <MathText forceInline>{entry.title || entry.link || "Untitled entry"}</MathText>
        </a>
        {entry.authors.length > 0 && (
          <p className="mt-0.5 text-xs text-muted truncate">
            {entry.authors.join(", ")}
          </p>
        )}
        {entry.summary !== "" && (
          <p className="mt-1 text-xs text-muted line-clamp-2 leading-relaxed">
            <MathText forceInline>{entry.summary}</MathText>
          </p>
        )}
        {entry.published !== "" && (
          <MonoLabel className="mt-1 normal-case">{entry.published}</MonoLabel>
        )}
      </div>
      <div className="flex flex-col items-end gap-1 shrink-0">
        <div className="flex items-center gap-3">
          <div className="flex items-center gap-2.5">
            {entry.arxiv_id !== null && entry.version !== null && (
              <button
                type="button"
                className="text-xs font-medium text-accent hover:underline"
                onClick={handlePreview}
              >
                PDF →
              </button>
            )}
            {entry.arxiv_id !== null && (
              (() => {
                const arxivId = entry.arxiv_id;
                return (
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={alreadySaved || (saveState !== "idle" && saveState !== "error")}
                    onClick={() => handleSave(arxivId)}
                  >
                    {(alreadySaved || saveState === "saved") ? "Saved" : saveState === "saving" ? "Saving…" : "Save"}
                  </Button>
                );
              })()
            )}
          </div>
          {entry.arxiv_id !== null && entry.version !== null && (
            <DismissControls
              dismissing={dismissing}
              onDismiss={() => handleDismiss(entry.arxiv_id as string, entry.version as number)}
              onRequestPermanentDismiss={() => setConfirmPermanent(true)}
            />
          )}
        </div>
        {saveState === "error" && (
          <p className="text-xs" style={{ color: "var(--color-danger)" }}>
            Save failed
          </p>
        )}
        {dismissError && (
          <p className="text-xs" style={{ color: "var(--color-danger)" }}>
            Dismiss failed
          </p>
        )}
      </div>
      <Dialog
        open={confirmPermanent}
        onClose={() => setConfirmPermanent(false)}
        title="Never show this paper again?"
      >
        <p className="text-sm text-muted mb-4">
          This hides every version of this paper from the home feed going forward.
        </p>
        <div className="flex justify-end gap-2">
          <Button variant="ghost" size="sm" onClick={() => setConfirmPermanent(false)}>
            Cancel
          </Button>
          <Button
            variant="danger"
            size="sm"
            onClick={() => {
              setConfirmPermanent(false);
              if (entry.arxiv_id !== null && entry.version !== null) {
                void handleDismiss(entry.arxiv_id, entry.version, true);
              }
            }}
          >
            Never show again
          </Button>
        </div>
      </Dialog>
    </div>
  );
}

function FeedSection({ url }: { url: string }) {
  const queryClient = useQueryClient();
  const { data, isLoading, error } = useQuery({
    queryKey: ["home-feed", url],
    queryFn: () => getFeed(url),
    staleTime: 5 * 60 * 1000,
  });

  const entries = data?.entries ?? [];
  const shown = entries.slice(0, FEED_ENTRY_LIMIT);
  const savedArxivIds = new Set(data?.saved_arxiv_ids ?? []);

  return (
    <section>
      <SectionTitle className="text-base mb-4">
        {data?.title || "Home feed"}
      </SectionTitle>
      {isLoading ? (
        <Card variant="inset">
          <div className="flex justify-center py-6">
            <Spinner size={20} />
          </div>
        </Card>
      ) : error ? (
        <Card variant="inset" className="text-center">
          <p className="text-sm" style={{ color: "var(--color-danger)" }}>
            {errText(error, "Failed to load feed")}
          </p>
          <p className="mt-1 text-xs text-muted">
            Feed URL is set in Settings → Library.
          </p>
        </Card>
      ) : entries.length === 0 ? (
        <Card variant="inset" className="text-center">
          <p className="text-muted text-sm">The feed has no entries.</p>
        </Card>
      ) : (
        <>
          <Card>
            <div className="py-1">
              {shown.map((entry, i) => (
                <FeedRow
                  key={`${entry.link}-${i}`}
                  entry={entry}
                  alreadySaved={entry.arxiv_id != null && savedArxivIds.has(entry.arxiv_id)}
                  onDismissed={() => queryClient.invalidateQueries({ queryKey: ["home-feed", url] })}
                />
              ))}
            </div>
          </Card>
          {entries.length > shown.length && (
            <p className="py-2 text-center text-xs text-muted">
              Showing {shown.length} of {entries.length} entries
            </p>
          )}
        </>
      )}
    </section>
  );
}

function tagCounts(papers: Paper[]): Array<{ tag: string; count: number }> {
  const counts = new Map<string, number>();
  for (const paper of papers) {
    for (const tag of paper.tags) {
      counts.set(tag, (counts.get(tag) ?? 0) + 1);
    }
  }
  return [...counts.entries()]
    .map(([tag, count]) => ({ tag, count }))
    .sort((a, b) => b.count - a.count)
    .slice(0, 8);
}

export default function HomePage() {
  const navigate = useNavigate();
  const { data, isLoading, error } = useQuery({
    queryKey: ["stats"],
    queryFn: getStats,
  });
  const { data: settings } = useQuery({
    queryKey: ["settings"],
    queryFn: getSettings,
  });
  const feedUrl =
    typeof settings?.home_feed_url === "string" ? settings.home_feed_url.trim() : "";

  return (
    <div className="p-8 space-y-8">

      {isLoading ? (
        <div className="flex items-center justify-center h-full">
          <Spinner size={28} />
        </div>
      ) : error ? (
        <div className="flex items-center justify-center h-full">
          <p className="text-sm" style={{ color: "var(--color-danger)" }}>
            {errText(error, "Failed to load stats")}
          </p>
        </div>
      ) : (
        <>
          <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-5 gap-3">
            <StatCard
              label="Papers"
              value={data?.paper_count}
              hint="in library"
              to="/library"
            />
            <StatCard
              label="PDFs"
              value={data?.pdf_count}
              hint="downloaded"
              to="/library"
            />
            <StatCard
              label="Coverage"
              value={(() => {
                const paperCount = data?.paper_count;
                const pdfCount = data?.pdf_count;
                return paperCount !== undefined && paperCount > 0 && pdfCount !== undefined
                  ? `${Math.round((pdfCount / paperCount) * 100)}%`
                  : undefined;
              })()}
              hint="pdf per paper"
            />
            <StatCard
              label="Categories"
              value={data?.category_count}
              hint="distinct"
            />
            <StatCard
              label="Tags"
              value={data?.tag_count}
              hint="applied"
              to="/tags"
            />
          </div>
          {feedUrl !== "" && <FeedSection url={feedUrl} />}

          {(() => {
            const recentPapers = data?.recent_papers?.slice(0, 10) ?? [];
            const topTags = tagCounts(recentPapers);
            const maxTagCount = topTags.length > 0 ? topTags[0].count : 0;

            return (
              <div className="grid grid-cols-1 lg:grid-cols-[1.4fr_1fr] gap-6">
                <section>
                  <SectionTitle className="text-base mb-4">Recent papers</SectionTitle>
                  {recentPapers.length === 0 ? (
                    <Card variant="inset" className="text-center">
                      <p className="text-muted text-sm">
                        No papers yet. Add some from the Library or Search pages.
                      </p>
                    </Card>
                  ) : (
                    <PaperList papers={recentPapers} className="space-y-3" />
                  )}
                </section>

                <section className="lg:sticky lg:top-8 self-start">
                  <SectionTitle className="text-base mb-4">Tags —— Recent Papers</SectionTitle>
                  {topTags.length === 0 ? (
                    <Card variant="inset" className="text-center">
                      <p className="text-muted text-sm">
                        No tags on recent papers yet.
                      </p>
                    </Card>
                  ) : (
                    <Card className="flex flex-col gap-3">
                      {topTags.map(({ tag, count }) => (
                        <button
                          key={tag}
                          type="button"
                          className="flex flex-col gap-1 text-left group"
                          onClick={() => navigate("/tags")}
                        >
                          <div className="flex items-baseline justify-between gap-2">
                            <span className="text-[13px] text-text truncate group-hover:text-accent transition-colors">
                              {tag}
                            </span>
                            <MonoLabel className="normal-case shrink-0">{count}</MonoLabel>
                          </div>
                          <div className="h-1.5 rounded-full bg-surface2 overflow-hidden">
                            <div
                              className="h-full rounded-full"
                              style={{
                                width: `${(count / maxTagCount) * 100}%`,
                                backgroundColor: "var(--color-accent)",
                              }}
                            />
                          </div>
                        </button>
                      ))}
                    </Card>
                  )}
                </section>
              </div>
            );
          })()}
        </>
      )}
    </div>
  );
}
