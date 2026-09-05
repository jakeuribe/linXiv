import { useEffect, useRef, useState } from "react";
import { TextSelect } from "lucide-react";
import { Dialog } from "../ui/dialog";
import { Input, Textarea } from "../ui/input";
import { Button } from "../ui/button";
import type { Paper } from "../../types/api";
import { repairPaper } from "../../api/papers";
import { formSubmitOnCtrlEnter } from "../../lib/submitShortcut";
import { errText } from "../../lib/errText";
import { getPdfSelectionText } from "../../lib/pdfSelection";

// Live text of the current selection inside the PDF pane ("" when none).
// The dialog is non-modal, so the user can select in the PDF while it's open.
function usePdfSelection() {
  const [text, setText] = useState("");
  useEffect(() => {
    const update = () => setText(getPdfSelectionText());
    update();
    document.addEventListener("selectionchange", update);
    return () => document.removeEventListener("selectionchange", update);
  }, []);
  return text;
}

function PullFromPdf({
  selection,
  onPull,
}: {
  selection: string;
  onPull: (text: string) => void;
}) {
  return (
    <button
      type="button"
      disabled={!selection}
      // Keep the PDF selection alive through the click (mousedown would clear it).
      onMouseDown={(e) => e.preventDefault()}
      onClick={() => onPull(selection)}
      className="ml-auto rounded p-0.5 transition-colors hover:bg-[var(--color-border)] disabled:opacity-30"
      style={{ color: "var(--color-muted)" }}
      title={selection ? "Insert text selected in the PDF" : "Select text in the PDF first"}
      aria-label="Insert text selected in the PDF"
    >
      <TextSelect size={13} />
    </button>
  );
}

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
  const pdfSelection = usePdfSelection();
  const titleRef = useRef<HTMLTextAreaElement>(null);

  // Fill when the field is empty, append with the field's separator otherwise.
  // Trailing whitespace/an already-typed separator is absorbed so "Jane; " +
  // pull never yields "Jane; ; John".
  const pull =
    (set: React.Dispatch<React.SetStateAction<string>>, sep = " ") =>
    (text: string) =>
      set((v) => {
        const base = v.replace(/\s+$/, "");
        if (!base) return text;
        const core = sep.trim();
        const joiner = core && base.endsWith(core) ? " " : sep;
        return base + joiner + text;
      });

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
    <Dialog
      open={true}
      onClose={onClose}
      title="Edit Paper"
      size="xl"
      modal={false}
      // Steer the mount focus: default lands on the first tabbable (a pull
      // icon), so initial typing goes nowhere. Focus the Title field — unless
      // a PDF selection already exists, which focusing an editable control
      // would collapse; then leave focus alone so the selection survives.
      onOpenAutoFocus={(e) => {
        e.preventDefault();
        if (!getPdfSelectionText()) titleRef.current?.focus();
      }}
    >
      <form onSubmit={handleSubmit} onKeyDown={formSubmitOnCtrlEnter} className="flex flex-col" onInput={() => setError(null)}>
        {/* Scrollable fields */}
        <div className="flex flex-col gap-4 overflow-y-auto max-h-[60vh] pr-1">
          <div className="flex flex-col gap-1.5">
            <label className="flex items-center gap-1 text-xs font-medium" style={{ color: "var(--color-muted)" }}>
              Title <span style={{ color: "var(--color-danger)" }}>*</span>
              <PullFromPdf selection={pdfSelection} onPull={pull(setTitle)} />
            </label>
            <Textarea
              ref={titleRef}
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              placeholder="e.g. Attention Is All You Need"
              className="min-h-[60px]"
              aria-required="true"
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <label className="flex items-center gap-1 text-xs font-medium" style={{ color: "var(--color-muted)" }}>
              Authors <span style={{ color: "var(--color-danger)" }}>*</span>{" "}
              <span className="font-normal">(semicolon-separated)</span>
              <PullFromPdf selection={pdfSelection} onPull={pull(setAuthors, "; ")} />
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
              <label className="flex items-center gap-1 text-xs font-medium" style={{ color: "var(--color-muted)" }}>
                Category
                <PullFromPdf selection={pdfSelection} onPull={pull(setCategory)} />
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
              <label className="flex items-center gap-1 text-xs font-medium" style={{ color: "var(--color-muted)" }}>
                DOI
                <PullFromPdf selection={pdfSelection} onPull={pull(setDoi)} />
              </label>
              <Input
                value={doi}
                onChange={(e) => setDoi(e.target.value)}
                placeholder="e.g. 10.48550/arXiv.1706.03762"
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <label className="flex items-center gap-1 text-xs font-medium" style={{ color: "var(--color-muted)" }}>
                URL
                <PullFromPdf selection={pdfSelection} onPull={pull(setUrl)} />
              </label>
              <Input
                value={url}
                onChange={(e) => setUrl(e.target.value)}
                placeholder="https://…"
              />
            </div>
          </div>

          <div className="flex flex-col gap-1.5">
            <label className="flex items-center gap-1 text-xs font-medium" style={{ color: "var(--color-muted)" }}>
              Abstract
              <PullFromPdf selection={pdfSelection} onPull={pull(setSummary)} />
            </label>
            <Textarea
              value={summary}
              onChange={(e) => setSummary(e.target.value)}
              placeholder="Paper abstract…"
              className="min-h-[100px]"
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <label className="flex items-center gap-1 text-xs font-medium" style={{ color: "var(--color-muted)" }}>
              Tags <span className="font-normal">(comma-separated)</span>
              <PullFromPdf selection={pdfSelection} onPull={pull(setTags, ", ")} />
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
