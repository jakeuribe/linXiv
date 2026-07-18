import { useMemo, useState } from "react";
import { useParams, useNavigate, Link } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { getAllTags, getTagDetail } from "../api/tags";
import { Spinner } from "../components/ui/spinner";
import { Button } from "../components/ui/button";
import { Badge } from "../components/ui/badge";
import { SortHeader, nextSort, type SortDir } from "../components/ui/sort-header";
import { PaperCard } from "../components/papers/PaperCard";
import { ProjectCard } from "../components/projects/ProjectCard";

export default function TagPage() {
  const { label } = useParams<{ label?: string }>();

  if (label) {
    return <TagDetailView label={label} />;
  }
  return <TagIndexView />;
}

// ---------------------------------------------------------------------------
// Tag index: all tags
// ---------------------------------------------------------------------------

type TagSortKey = "label" | "paper_count";

function TagIndexView() {
  const navigate = useNavigate();
  const [sort, setSort] = useState<{ key: TagSortKey; dir: SortDir }>({
    key: "paper_count",
    dir: "desc",
  });

  const { data: tags = [], isLoading, error } = useQuery({
    queryKey: ["tags"],
    queryFn: getAllTags,
  });

  const sorted = useMemo(() => {
    const dir = sort.dir === "asc" ? 1 : -1;
    return [...tags].sort((a, b) => {
      const primary =
        sort.key === "label"
          ? a.label.localeCompare(b.label)
          : a.paper_count - b.paper_count;
      return primary !== 0 ? primary * dir : a.label.localeCompare(b.label);
    });
  }, [tags, sort]);

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
          Failed to load tags.
        </p>
      </div>
    );
  }

  const toggle = (key: TagSortKey) =>
    setSort((s) => nextSort(s, key, key === "paper_count" ? "desc" : "asc"));

  return (
    <div className="max-w-5xl mx-auto px-6 py-8 space-y-6">
      <div>
        <h1 className="font-display text-[27px] font-semibold leading-tight tracking-[-0.015em] text-text">Tags</h1>
        <p className="text-sm mt-1" style={{ color: "var(--color-muted)" }}>
          {tags.length} tag{tags.length !== 1 ? "s" : ""} across your library
        </p>
      </div>

      {tags.length === 0 ? (
        <p className="text-sm" style={{ color: "var(--color-muted)" }}>
          No tags yet. Add tags to papers to see them here.
        </p>
      ) : (
        <table className="w-full text-sm border-collapse">
          <thead>
            <tr className="border-b" style={{ borderColor: "var(--color-border)" }}>
              <SortHeader
                label="Tag"
                active={sort.key === "label"}
                dir={sort.dir}
                onSort={() => toggle("label")}
              />
              <SortHeader
                label="Papers"
                active={sort.key === "paper_count"}
                dir={sort.dir}
                onSort={() => toggle("paper_count")}
                align="right"
              />
            </tr>
          </thead>
          <tbody>
            {sorted.map((tag) => {
              const to = `/tags/${encodeURIComponent(tag.label)}`;
              return (
              <tr
                key={tag.label}
                onClick={() => navigate(to)}
                className="border-b cursor-pointer hover:bg-[var(--color-panel)] transition-colors"
                style={{ borderColor: "var(--color-border)" }}
              >
                <td className="py-2.5">
                  <Link to={to} className="inline-block" onClick={(e) => e.stopPropagation()}>
                    <Badge
                      size="sm"
                      style={{
                        borderColor: "var(--color-accent)",
                        color: "var(--color-accent)",
                        backgroundColor: "color-mix(in srgb, var(--color-accent) 12%, transparent)",
                      }}
                    >
                      {tag.label}
                    </Badge>
                  </Link>
                </td>
                <td className="py-2.5 text-right tabular-nums" style={{ color: "var(--color-muted)" }}>
                  {tag.paper_count}
                </td>
              </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Tag detail: papers + projects with this tag
// ---------------------------------------------------------------------------

interface TagDetailViewProps {
  label: string;
}

function TagDetailView({ label }: TagDetailViewProps) {
  const navigate = useNavigate();
  // Normalize to lowercase so /tags/Python and /tags/python share a cache entry.
  const normalizedLabel = label.toLowerCase();

  const { data, isLoading, error } = useQuery({
    queryKey: ["tag", normalizedLabel],
    queryFn: () => getTagDetail(normalizedLabel),
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
          Failed to load tag details.
        </p>
      </div>
    );
  }

  const papers = data?.papers ?? [];
  const projects = data?.projects ?? [];
  // Use the canonical label returned by the backend (case-preserved from the database).
  const displayLabel = data?.label ?? label;

  return (
    <div className="max-w-5xl mx-auto px-6 py-8 space-y-8">
      {/* Header */}
      <div className="flex items-start gap-4">
        <Button
          variant="ghost"
          size="sm"
          onClick={() => window.history.length > 1 ? navigate(-1) : navigate("/tags")}
        >
          ← Back
        </Button>
      </div>

      <div className="flex items-center gap-3">
        <Badge
          size="md"
          style={{
            borderColor: "var(--color-accent)",
            color: "var(--color-accent)",
            backgroundColor: "color-mix(in srgb, var(--color-accent) 12%, transparent)",
          }}
        >
          {displayLabel}
        </Badge>
        <span className="text-sm" style={{ color: "var(--color-muted)" }}>
          {papers.length} paper{papers.length !== 1 ? "s" : ""}
          {projects.length > 0 &&
            `, ${projects.length} project${projects.length !== 1 ? "s" : ""}`}
        </span>
      </div>

      {/* Papers section */}
      <section className="space-y-3">
        <h2 className="text-base font-semibold text-text">
          Papers{papers.length > 0 ? ` (${papers.length})` : ""}
        </h2>
        {papers.length === 0 ? (
          <p className="text-sm" style={{ color: "var(--color-muted)" }}>
            No papers with this tag.
          </p>
        ) : (
          <div className="flex flex-col gap-3">
            {papers.map((paper) => (
              <PaperCard
                key={paper.source_id}
                paper={paper}
                onNavigate={(sfk) => navigate(`/library/${sfk}`)}
              />
            ))}
          </div>
        )}
      </section>

      {/* Projects section */}
      {projects.length > 0 && (
        <section className="space-y-3">
          <h2 className="text-base font-semibold text-text">
            Projects ({projects.length})
          </h2>
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
            {projects.map((project) => (
              <ProjectCard
                key={project.id}
                project={project}
                onClick={() => navigate(`/projects/${project.id}`)}
              />
            ))}
          </div>
        </section>
      )}
    </div>
  );
}
