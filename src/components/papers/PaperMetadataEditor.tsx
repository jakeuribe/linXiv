import { useState } from "react";
import { Dialog } from "../ui/dialog";
import { Input, Textarea } from "../ui/input";
import { Button } from "../ui/button";
import type { Paper } from "../../types/api";
import { repairPaper } from "../../api/papers";
import { formSubmitOnCtrlEnter } from "../../lib/submitShortcut";
import { errText } from "../../lib/errText";

interface PaperMetadataEditorProps {
  onClose: () => void;
  paper: Paper;
  onSaved: (updated: Paper) => void;
}

export function PaperMetadataEditor({
  onClose,
  paper,
  onSaved,
}: PaperMetadataEditorProps) {
  // Lazy initializers seed from paper at mount time. Conditional mount in the
  // parent ensures a fresh instance (and fresh values) each time the editor opens.
  const [title, setTitle] = useState(() => paper.title ?? "");
  const [authors, setAuthors] = useState(() => paper.authors.join("; "));
  const [published, setPublished] = useState(() => paper.published ? paper.published.split("T")[0] : "");
  const [category, setCategory] = useState(() => paper.category ?? "");
  const [doi, setDoi] = useState(() => paper.doi ?? "");
  const [url, setUrl] = useState(() => paper.url ?? "");
  const [summary, setSummary] = useState(() => paper.summary ?? "");
  const [tags, setTags] = useState(() => (paper.tags ?? []).join(", "));
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (submitting) return;
    if (!title.trim()) {
      setError("Title is required.");
      return;
    }
    if (!published) {
      setError("Published date is required.");
      return;
    }
    const authorList = [...new Set(authors.split(";").map((a) => a.trim()).filter(Boolean))];
    if (authorList.length === 0) {
      setError("At least one author is required.");
      return;
    }
    // Repair is keyed by source_fk. PaperDetails leaves it optional in Rust
    // (the struct doubles as a write shape), but every paper read back from
    // the API has one — bail rather than PATCH a made-up id.
    if (paper.source_fk === null) {
      setError("This paper has no stable id to repair.");
      return;
    }
    setSubmitting(true);
    setError(null);
    try {
      const tagList = [...new Set(tags.split(",").map((t) => t.trim()).filter(Boolean))];
      const updated = await repairPaper(paper.source_fk, {
        title: title.trim(),
        authors: authorList,
        published,
        summary: summary.trim(),
        category: category.trim() || null,
        doi: doi.trim() || null,
        url: url.trim() || null,
        tags: tagList.length > 0 ? tagList : null,
      });
      onSaved(updated);
      onClose();
    } catch (err) {
      setError(errText(err, "Failed to save changes"));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Dialog open={true} onClose={onClose} title="Edit Paper" size="xl">
      <form onSubmit={handleSubmit} onKeyDown={formSubmitOnCtrlEnter} className="flex flex-col" onInput={() => setError(null)}>
        {/* Scrollable fields */}
        <div className="flex flex-col gap-4 overflow-y-auto max-h-[60vh] pr-1">
          <div className="flex flex-col gap-1.5">
            <label className="text-xs font-medium" style={{ color: "var(--color-muted)" }}>
              Title <span style={{ color: "var(--color-danger)" }}>*</span>
            </label>
            <Textarea
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              placeholder="e.g. Attention Is All You Need"
              className="min-h-[60px]"
              autoFocus
              aria-required="true"
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <label className="text-xs font-medium" style={{ color: "var(--color-muted)" }}>
              Authors <span style={{ color: "var(--color-danger)" }}>*</span>{" "}
              <span className="font-normal">(semicolon-separated)</span>
            </label>
            <Textarea
              value={authors}
              onChange={(e) => setAuthors(e.target.value)}
              placeholder="e.g. Ashish Vaswani; Noam Shazeer; Niki Parmar"
              className="min-h-[60px]"
            />
          </div>

          <div className="grid grid-cols-2 gap-3">
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-medium" style={{ color: "var(--color-muted)" }}>
                Published <span style={{ color: "var(--color-danger)" }}>*</span>
              </label>
              <Input
                type="date"
                value={published}
                onChange={(e) => setPublished(e.target.value)}
                aria-required="true"
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-medium" style={{ color: "var(--color-muted)" }}>
                Category
              </label>
              <Input
                value={category}
                onChange={(e) => setCategory(e.target.value)}
                placeholder="e.g. cs.LG"
              />
            </div>
          </div>

          <div className="grid grid-cols-2 gap-3">
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-medium" style={{ color: "var(--color-muted)" }}>
                DOI
              </label>
              <Input
                value={doi}
                onChange={(e) => setDoi(e.target.value)}
                placeholder="e.g. 10.48550/arXiv.1706.03762"
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-medium" style={{ color: "var(--color-muted)" }}>
                URL
              </label>
              <Input
                value={url}
                onChange={(e) => setUrl(e.target.value)}
                placeholder="https://…"
              />
            </div>
          </div>

          <div className="flex flex-col gap-1.5">
            <label className="text-xs font-medium" style={{ color: "var(--color-muted)" }}>
              Abstract
            </label>
            <Textarea
              value={summary}
              onChange={(e) => setSummary(e.target.value)}
              placeholder="Paper abstract…"
              className="min-h-[100px]"
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <label className="text-xs font-medium" style={{ color: "var(--color-muted)" }}>
              Tags <span className="font-normal">(comma-separated)</span>
            </label>
            <Input
              value={tags}
              onChange={(e) => setTags(e.target.value)}
              placeholder="e.g. transformers, nlp"
            />
          </div>
        </div>

        {/* Fixed footer */}
        <div className="flex items-center justify-between gap-4 pt-4 mt-1 border-t border-[var(--color-border)]">
          <div
            aria-live="polite"
            className="text-xs min-w-0 truncate"
            style={{ color: "var(--color-danger)" }}
          >
            {error}
          </div>
          <div className="flex gap-2 shrink-0">
            <Button type="button" variant="muted" onClick={onClose} disabled={submitting}>
              Cancel
            </Button>
            <Button type="submit" variant="primary" disabled={submitting || !title.trim() || !published || !authors.trim()}>
              {submitting ? "Saving…" : "Save Changes"}
            </Button>
          </div>
        </div>
      </form>
    </Dialog>
  );
}
