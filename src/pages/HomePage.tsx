import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import { getStats } from "../api/settings";
import { Spinner } from "../components/ui/spinner";
import { PaperCard } from "../components/papers/PaperCard";
import { Card, MonoLabel, SectionTitle } from "../components/ui/card";
import type { Paper } from "../types/api";

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

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-full">
        <Spinner size={28} />
      </div>
    );
  }

  if (error) {
    return (
      <div className="flex items-center justify-center h-full">
        <p className="text-sm" style={{ color: "var(--color-danger)" }}>
          {error instanceof Error ? error.message : "Failed to load stats"}
        </p>
      </div>
    );
  }

  const recentPapers = data?.recent_papers?.slice(0, 10) ?? [];
  const topTags = tagCounts(recentPapers);
  const paperCount = data?.paper_count;
  const pdfCount = data?.pdf_count;
  const coverage =
    paperCount !== undefined && paperCount > 0 && pdfCount !== undefined
      ? `${Math.round((pdfCount / paperCount) * 100)}%`
      : undefined;
  const maxTagCount = topTags.length > 0 ? topTags[0].count : 0;

  return (
    <div className="p-8 space-y-8 overflow-y-auto">
      <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-5 gap-3">
        <StatCard
          label="Papers"
          value={paperCount}
          hint="in library"
          to="/library"
        />
        <StatCard
          label="PDFs"
          value={pdfCount}
          hint="downloaded"
          to="/library"
        />
        <StatCard
          label="Coverage"
          value={coverage}
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

      <div className="grid grid-cols-1 lg:grid-cols-[1.4fr_1fr] gap-6">
        <section>
          <SectionTitle className="text-base mb-4">Recent papers</SectionTitle>
          {recentPapers.length === 0 ? (
            <Card inset className="text-center">
              <p className="text-muted text-sm">
                No papers yet. Add some from the Library or Search pages.
              </p>
            </Card>
          ) : (
            <div className="space-y-3">
              {recentPapers.map((paper) => (
                <PaperCard
                  key={paper.source_id}
                  paper={paper}
                  onNavigate={(id) => navigate(`/library/${id}`)}
                />
              ))}
            </div>
          )}
        </section>

        <section>
          <SectionTitle className="text-base mb-4">Tags — recent papers</SectionTitle>
          {topTags.length === 0 ? (
            <Card inset className="text-center">
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
    </div>
  );
}
