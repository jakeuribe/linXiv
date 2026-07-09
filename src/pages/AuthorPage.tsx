import { useMemo, useState } from "react";
import { useParams, useNavigate, Link } from "react-router-dom";
import { useQuery, useQueryClient, useMutation } from "@tanstack/react-query";
import { listAuthors, getAuthor, updateAuthor, deleteAuthor, mergeAuthors } from "../api/authors";
import type { AuthorUpdateBody } from "../api/authors";
import { Spinner } from "../components/ui/spinner";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
import { SortHeader, nextSort, type SortDir } from "../components/ui/sort-header";
import { nameSortKey, type NameSortBy } from "../lib/authorName";
import { useUiStore } from "../stores/ui";
import { MathText } from "../lib/tex";
import { submitOnCtrlEnter } from "../lib/submitShortcut";

// Matches an author against a free-text query across full/first/last name and ORCID.
function authorMatchesQuery(a: { full_name?: string | null; first_name?: string | null; last_name?: string | null; orcid?: string | null }, query: string) {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return (
    a.full_name?.toLowerCase().includes(q) ||
    a.first_name?.toLowerCase().includes(q) ||
    a.last_name?.toLowerCase().includes(q) ||
    a.orcid?.toLowerCase().includes(q)
  );
}

export default function AuthorPage() {
  const { id } = useParams<{ id?: string }>();

  if (id === undefined) {
    return <AuthorIndexView />;
  }

  const authorId = Number(id);
  if (!Number.isInteger(authorId) || authorId <= 0) {
    return (
      <div className="flex items-center justify-center h-full">
        <p className="text-sm" style={{ color: "var(--color-danger)" }}>
          Invalid author ID.
        </p>
      </div>
    );
  }

  return <AuthorDetailView key={authorId} authorId={authorId} />;
}

// ---------------------------------------------------------------------------
// Index: list all authors
// ---------------------------------------------------------------------------

type AuthorSortKey = "name" | "paper_count";

function AuthorIndexView() {
  const navigate = useNavigate();
  const [search, setSearch] = useState("");
  const [sort, setSort] = useState<{ key: AuthorSortKey; dir: SortDir }>({
    key: "paper_count",
    dir: "desc",
  });
  // Which name part the "Author" column sorts by (full / first / last name).
  const [nameBy, setNameBy] = useState<NameSortBy>("full_name");
  const hideSingleAuthors = useUiStore((s) => s.hideSingleAuthors);
  const setHideSingleAuthors = useUiStore((s) => s.setHideSingleAuthors);

  const { data: authors = [], isLoading, error } = useQuery({
    queryKey: ["authors", { hideSingleAuthors }],
    queryFn: () => listAuthors(hideSingleAuthors),
  });

  const filtered = useMemo(() => {
    const matched = authors.filter((a) => authorMatchesQuery(a, search));
    const dir = sort.dir === "asc" ? 1 : -1;
    const keyed = matched.map((author) => ({ author, key: nameSortKey(author, nameBy) }));
    keyed.sort((a, b) => {
      const primary =
        sort.key === "name"
          ? a.key.localeCompare(b.key)
          : (a.author.paper_count ?? 0) - (b.author.paper_count ?? 0);
      return primary !== 0 ? primary * dir : a.key.localeCompare(b.key);
    });
    return keyed.map((k) => k.author);
  }, [authors, search, sort, nameBy]);

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
          Failed to load authors.
        </p>
      </div>
    );
  }

  return (
    <div className="max-w-4xl mx-auto px-6 py-8 space-y-6">
      <div>
        <h1 className="font-display text-[27px] font-semibold leading-tight tracking-[-0.015em] text-text">
          Authors
        </h1>
        <p className="text-sm mt-1" style={{ color: "var(--color-muted)" }}>
          {authors.length} author{authors.length !== 1 ? "s" : ""}
          {hideSingleAuthors ? " with multiple papers" : " in your library"}
        </p>
      </div>

      <Input
        placeholder="Filter by name or ORCID…"
        value={search}
        onChange={(e) => setSearch(e.target.value)}
        className="w-full"
      />

      <label
        className="flex items-center gap-2 text-sm cursor-pointer select-none w-fit"
        style={{ color: "var(--color-muted)" }}
      >
        <input
          type="checkbox"
          checked={hideSingleAuthors}
          onChange={(e) => setHideSingleAuthors(e.target.checked)}
        />
        Hide single-paper authors
      </label>

      <label
        className="flex items-center gap-2 text-sm w-fit"
        style={{ color: "var(--color-muted)" }}
      >
        Sort names by
        <select
          value={nameBy}
          onChange={(e) => {
            setNameBy(e.target.value as NameSortBy);
            // Make the choice take effect even if currently sorted by papers.
            setSort((s) => (s.key === "name" ? s : { key: "name", dir: "asc" }));
          }}
          className="rounded-md border px-2 py-1 bg-transparent"
          style={{ borderColor: "var(--color-border)", color: "var(--color-text)" }}
        >
          <option value="full_name">Full name</option>
          <option value="first_name">First name</option>
          <option value="last_name">Last name</option>
        </select>
      </label>

      {filtered.length === 0 ? (
        <p className="text-sm" style={{ color: "var(--color-muted)" }}>
          {search
            ? "No authors match your filter."
            : hideSingleAuthors
            ? "No authors with more than one paper — uncheck “Hide single-paper authors” to show the rest."
            : "No authors yet."}
        </p>
      ) : (
        <table className="w-full text-sm border-collapse">
          <thead>
            <tr className="border-b" style={{ borderColor: "var(--color-border)" }}>
              <SortHeader
                label="Author"
                active={sort.key === "name"}
                dir={sort.dir}
                onSort={() => setSort((s) => nextSort(s, "name", "asc"))}
              />
              <SortHeader
                label="Papers"
                active={sort.key === "paper_count"}
                dir={sort.dir}
                onSort={() => setSort((s) => nextSort(s, "paper_count", "desc"))}
                align="right"
              />
            </tr>
          </thead>
          <tbody>
            {filtered.map((author) => {
              const to = `/authors/${author.author_id}`;
              return (
              <tr
                key={author.author_id}
                onClick={() => navigate(to)}
                className="border-b cursor-pointer hover:bg-[var(--color-panel)] transition-colors"
                style={{ borderColor: "var(--color-border)" }}
              >
                <td className="py-2.5 font-medium" style={{ color: "var(--color-text)" }}>
                  <Link to={to} className="block" onClick={(e) => e.stopPropagation()}>
                    {author.full_name ?? "(unnamed)"}
                  </Link>
                </td>
                <td className="py-2.5 text-right tabular-nums" style={{ color: "var(--color-muted)" }}>
                  {author.paper_count ?? 0}
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
// Detail: single author edit + linked papers
// ---------------------------------------------------------------------------

interface AuthorDetailViewProps {
  authorId: number;
}

function AuthorDetailView({ authorId }: AuthorDetailViewProps) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();

  const [editing, setEditing] = useState(false);
  const [form, setForm] = useState<AuthorUpdateBody>({});
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [mergeIds, setMergeIds] = useState<number[]>([]);
  const [mergeFilter, setMergeFilter] = useState("");
  const [mergeActive, setMergeActive] = useState(false);

  const { data: author, isLoading, error } = useQuery({
    queryKey: ["author", authorId],
    queryFn: () => getAuthor(authorId),
  });

  const updateMutation = useMutation({
    mutationFn: (body: AuthorUpdateBody) => updateAuthor(authorId, body),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["author", authorId] });
      queryClient.invalidateQueries({ queryKey: ["authors"] });
      setEditing(false);
    },
  });

  const deleteMutation = useMutation({
    mutationFn: () => deleteAuthor(authorId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["authors"] });
      navigate("/authors");
    },
    onError: (err: Error) => setDeleteError(err.message),
  });

  // Full author list for picking merge duplicates (excludes this author below).
  // Only fetched once the merge section is actually interacted with.
  const { data: allAuthors = [] } = useQuery({
    queryKey: ["authors", { hideSingleAuthors: false }],
    queryFn: () => listAuthors(false),
    enabled: mergeActive,
  });

  const mergeMutation = useMutation({
    mutationFn: () => mergeAuthors(authorId, mergeIds),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["author", authorId] });
      queryClient.invalidateQueries({ queryKey: ["authors"] });
      mergeIds.forEach((dupId) =>
        queryClient.removeQueries({ queryKey: ["author", dupId] })
      );
      setMergeIds([]);
    },
  });

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-full">
        <Spinner size={28} />
      </div>
    );
  }

  if (error || !author) {
    return (
      <div className="flex items-center justify-center h-full">
        <p className="text-sm" style={{ color: "var(--color-danger)" }}>
          Author not found.
        </p>
      </div>
    );
  }

  const authorDetail = author;

  function startEdit() {
    updateMutation.reset();
    setDeleteError(null);
    setForm({
      full_name:  authorDetail.full_name  ?? "",
      first_name: authorDetail.first_name ?? "",
      last_name:  authorDetail.last_name  ?? "",
      orcid:      authorDetail.orcid      ?? "",
    });
    setEditing(true);
  }

  function handleSave() {
    const updates: AuthorUpdateBody = {};
    if (form.full_name?.trim())  updates.full_name  = form.full_name.trim();
    if (form.first_name?.trim()) updates.first_name = form.first_name.trim();
    if (form.last_name?.trim())  updates.last_name  = form.last_name.trim();
    if (form.orcid?.trim())      updates.orcid      = form.orcid.trim();
    if (Object.keys(updates).length === 0) {
      setEditing(false);
      return;
    }
    updateMutation.mutate(updates);
  }

  return (
    <div className="max-w-4xl mx-auto px-6 py-8 space-y-8">
      {/* Back */}
      <Button
        variant="ghost"
        size="sm"
        onClick={() => (window.history.length > 1 ? navigate(-1) : navigate("/authors"))}
      >
        ← Back
      </Button>

      {/* Author fields */}
      <section className="space-y-4">
        <div className="flex items-center justify-between">
          <h1 className="text-xl font-semibold" style={{ color: "var(--color-text)" }}>
            {authorDetail.full_name ?? "(unnamed)"}
          </h1>
          {!editing && (
            <Button variant="outline" size="sm" onClick={startEdit}>
              Edit
            </Button>
          )}
        </div>

        {editing ? (
          <div
            className="space-y-3"
            onKeyDown={submitOnCtrlEnter(() => {
              if (!updateMutation.isPending) handleSave();
            })}
          >
            <LabeledField label="Full name">
              <Input
                value={form.full_name ?? ""}
                onChange={(e) => setForm((f) => ({ ...f, full_name: e.target.value }))}
                placeholder="Full name"
              />
            </LabeledField>
            <LabeledField label="First name">
              <Input
                value={form.first_name ?? ""}
                onChange={(e) => setForm((f) => ({ ...f, first_name: e.target.value }))}
                placeholder="First name"
              />
            </LabeledField>
            <LabeledField label="Last name">
              <Input
                value={form.last_name ?? ""}
                onChange={(e) => setForm((f) => ({ ...f, last_name: e.target.value }))}
                placeholder="Last name"
              />
            </LabeledField>
            <LabeledField label="ORCID">
              <Input
                value={form.orcid ?? ""}
                onChange={(e) => setForm((f) => ({ ...f, orcid: e.target.value }))}
                placeholder="0000-0000-0000-0000"
              />
            </LabeledField>

            <p className="text-xs" style={{ color: "var(--color-muted)" }}>
              Blank fields are ignored; clearing a value is not supported.
            </p>

            {updateMutation.error && (
              <p className="text-sm" style={{ color: "var(--color-danger)" }}>
                {(updateMutation.error as Error).message}
              </p>
            )}

            <div className="flex gap-2 pt-1">
              <Button
                size="sm"
                onClick={handleSave}
                disabled={updateMutation.isPending}
              >
                {updateMutation.isPending ? "Saving…" : "Save"}
              </Button>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => setEditing(false)}
                disabled={updateMutation.isPending}
              >
                Cancel
              </Button>
            </div>
          </div>
        ) : (
          <dl className="space-y-2">
            <FieldDisplay label="First name" value={authorDetail.first_name} />
            <FieldDisplay label="Last name" value={authorDetail.last_name} />
            <FieldDisplay label="ORCID" value={authorDetail.orcid} />
          </dl>
        )}
      </section>

      {/* Linked papers */}
      <section className="space-y-3">
        <h2 className="text-base font-semibold" style={{ color: "var(--color-text)" }}>
          Papers ({authorDetail.papers.length})
        </h2>
        <p className="text-xs" style={{ color: "var(--color-muted)" }}>
          Editing this author's name does not update the author list stored with each paper.
        </p>
        {authorDetail.papers.length === 0 ? (
          <p className="text-sm" style={{ color: "var(--color-muted)" }}>
            No papers linked to this author.
          </p>
        ) : (
          <div
            className="flex flex-col divide-y rounded-md overflow-hidden"
            style={{
              borderColor: "var(--color-border)",
              border: "1px solid var(--color-border)",
            }}
          >
            {authorDetail.papers.map((paper) => (
              <button
                key={paper.paper_id}
                type="button"
                className="flex items-start gap-3 px-4 py-3 text-left hover:opacity-80 transition-opacity"
                style={{ backgroundColor: "var(--color-panel)" }}
                onClick={() => navigate(`/library/${paper.source_fk}`)}
              >
                <span className="text-sm flex-1" style={{ color: "var(--color-text)" }}>
                  <MathText forceInline>{paper.title ?? paper.source_id}</MathText>
                </span>
                <span className="text-xs shrink-0 mt-0.5" style={{ color: "var(--color-muted)" }}>
                  v{paper.version}
                </span>
              </button>
            ))}
          </div>
        )}
      </section>

      {/* Merge duplicates */}
      <section className="space-y-3 pt-4" style={{ borderTop: "1px solid var(--color-border)" }}>
        <h2 className="text-base font-semibold" style={{ color: "var(--color-text)" }}>
          Merge duplicates
        </h2>
        <p className="text-xs" style={{ color: "var(--color-muted)" }}>
          Pick other records for the same person. Their papers move to this author and the
          duplicate records are deleted. This cannot be undone.
        </p>
        <Input
          placeholder="Filter authors…"
          value={mergeFilter}
          onChange={(e) => setMergeFilter(e.target.value)}
          onFocus={() => setMergeActive(true)}
          className="w-full"
        />
        <select
          multiple
          aria-label="Authors to merge into this author"
          size={Math.min(8, Math.max(3, allAuthors.length - 1))}
          value={mergeIds.map(String)}
          onFocus={() => setMergeActive(true)}
          onChange={(e) =>
            setMergeIds(Array.from(e.target.selectedOptions, (o) => Number(o.value)))
          }
          className="w-full rounded-md border px-2 py-1 bg-transparent text-sm"
          style={{ borderColor: "var(--color-border)", color: "var(--color-text)" }}
        >
          {allAuthors
            .filter((a) => a.author_id !== authorId)
            .filter((a) => authorMatchesQuery(a, mergeFilter))
            .slice(0, 50)
            .map((a) => (
              <option key={a.author_id} value={a.author_id}>
                {a.full_name ?? "(unnamed)"} ({a.paper_count ?? 0})
              </option>
            ))}
        </select>
        {mergeMutation.error && (
          <p className="text-sm" style={{ color: "var(--color-danger)" }}>
            {(mergeMutation.error as Error).message}
          </p>
        )}
        <Button
          variant="outline"
          size="sm"
          onClick={() => {
            const names = allAuthors
              .filter((a) => mergeIds.includes(a.author_id))
              .map((a) => a.full_name ?? "(unnamed)");
            if (window.confirm(`Merge ${names.join(", ")} into this author? This cannot be undone.`)) {
              mergeMutation.mutate();
            }
          }}
          disabled={mergeIds.length === 0 || mergeMutation.isPending}
        >
          {mergeMutation.isPending
            ? "Merging…"
            : `Merge${mergeIds.length ? ` ${mergeIds.length}` : ""} into this author`}
        </Button>
      </section>

      {/* Delete */}
      <section className="space-y-2 pt-4" style={{ borderTop: "1px solid var(--color-border)" }}>
        <h2 className="text-sm font-medium" style={{ color: "var(--color-danger)" }}>
          Danger zone
        </h2>
        {deleteError && (
          <p className="text-sm" style={{ color: "var(--color-danger)" }}>
            {deleteError}
          </p>
        )}
        <Button
          variant="outline"
          size="sm"
          onClick={() => {
            setDeleteError(null);
            deleteMutation.mutate();
          }}
          disabled={deleteMutation.isPending || authorDetail.papers.length > 0}
          style={
            authorDetail.papers.length === 0
              ? { borderColor: "var(--color-danger)", color: "var(--color-danger)" }
              : undefined
          }
        >
          {deleteMutation.isPending ? "Deleting…" : "Delete author"}
        </Button>
        {authorDetail.papers.length > 0 && (
          <p className="text-xs" style={{ color: "var(--color-muted)" }}>
            Cannot delete — author is linked to {authorDetail.papers.length} paper
            {authorDetail.papers.length !== 1 ? "s" : ""}.
          </p>
        )}
      </section>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Small layout helpers
// ---------------------------------------------------------------------------

function LabeledField({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex flex-col gap-1">
      <label className="text-xs font-medium" style={{ color: "var(--color-muted)" }}>
        {label}
      </label>
      {children}
    </div>
  );
}

function FieldDisplay({ label, value }: { label: string; value: string | null }) {
  return (
    <div className="flex gap-2 text-sm">
      <dt className="w-24 shrink-0 font-medium" style={{ color: "var(--color-muted)" }}>
        {label}
      </dt>
      <dd style={{ color: value ? "var(--color-text)" : "var(--color-muted)" }}>
        {value ?? "—"}
      </dd>
    </div>
  );
}
