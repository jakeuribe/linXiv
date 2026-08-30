import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { addPapers } from "../../api/projects";
import { listPapers } from "../../api/papers";
import { Button } from "../ui/button";
import { Dialog } from "../ui/dialog";
import { submitOnCtrlEnter } from "../../lib/submitShortcut";
import { Input } from "../ui/input";
import { Spinner } from "../ui/spinner";
import { MathText } from "../../lib/tex";
import {
  invalidateProjectMembershipQueries,
  partialFailureMessage,
} from "../../lib/paperMutations";
import { errText } from "../../lib/errText";

interface AddPapersDialogProps {
  open: boolean;
  onClose: () => void;
  projectId: number;
  existingSourceIds: string[];
}

export function AddPapersDialog({
  open,
  onClose,
  projectId,
  existingSourceIds,
}: AddPapersDialogProps) {
  const queryClient = useQueryClient();
  const [search, setSearch] = useState("");
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const { data: papersData, isLoading } = useQuery({
    queryKey: ["papers"],
    queryFn: () => listPapers(),
    enabled: open,
  });

  // Reset on open
  useEffect(() => {
    if (open) {
      setSearch("");
      setSelectedIds(new Set());
      setError(null);
    }
  }, [open]);

  const candidates = (papersData?.papers ?? []).filter(
    (p) => !existingSourceIds.includes(p.source_id)
  );

  const filtered = candidates.filter((p) =>
    search.trim()
      ? p.title.toLowerCase().includes(search.toLowerCase()) ||
        p.source_id.toLowerCase().includes(search.toLowerCase())
      : true
  );

  function toggleId(id: string) {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  }

  async function handleSubmit() {
    if (submitting) return;
    if (selectedIds.size === 0) return;
    setSubmitting(true);
    setError(null);
    try {
      const ids = [...selectedIds];
      const failed = await addPapers({ projectId, sourceIds: ids });
      // Also covers ["projects"], which backs the note scope picker / badges.
      await invalidateProjectMembershipQueries(queryClient);
      if (failed.length > 0) {
        setSelectedIds(new Set(failed));
        setError(partialFailureMessage(failed.length, ids.length));
      } else {
        onClose();
      }
    } catch (err) {
      setError(errText(err, "Failed to add papers"));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Dialog open={open} onClose={onClose} title="Add Papers">
      <div className="flex flex-col gap-3">
        <Input
          placeholder="Search papers..."
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          autoFocus
        />

        <div
          className="overflow-y-auto rounded-md border border-[var(--color-border)]"
          style={{ maxHeight: 280, backgroundColor: "var(--color-bg)" }}
          onKeyDown={submitOnCtrlEnter(handleSubmit)}
        >
          {isLoading ? (
            <div className="flex items-center justify-center p-6">
              <Spinner size={20} />
            </div>
          ) : filtered.length === 0 ? (
            <p
              className="p-4 text-sm text-center"
              style={{ color: "var(--color-muted)" }}
            >
              {candidates.length === 0
                ? "All library papers are already in this project"
                : "No papers match your search"}
            </p>
          ) : (
            filtered.map((paper) => (
              <label
                key={paper.source_id}
                className="flex items-start gap-3 px-3 py-2.5 cursor-pointer transition-colors hover:bg-[var(--color-panel)]"
                style={{ borderBottom: "1px solid var(--color-border)" }}
              >
                <input
                  type="checkbox"
                  checked={selectedIds.has(paper.source_id)}
                  onChange={() => toggleId(paper.source_id)}
                  className="mt-0.5 accent-[var(--color-accent)] shrink-0"
                />
                <div className="flex flex-col gap-0.5 min-w-0">
                  <span
                    className="text-sm font-medium leading-snug line-clamp-2"
                    style={{ color: "var(--color-text)" }}
                  >
                    <MathText forceInline>{paper.title}</MathText>
                  </span>
                  <span
                    className="text-xs truncate"
                    style={{ color: "var(--color-muted)" }}
                    title={paper.source_id}
                  >
                    {paper.authors.join(", ") || paper.source_id}
                  </span>
                </div>
              </label>
            ))
          )}
        </div>

        {error && (
          <p className="text-xs" style={{ color: "var(--color-danger)" }}>
            {error}
          </p>
        )}

        <div className="flex items-center justify-between pt-1">
          <span className="text-xs" style={{ color: "var(--color-muted)" }}>
            {selectedIds.size > 0 ? `${selectedIds.size} selected` : ""}
          </span>
          <div className="flex gap-2">
            <Button type="button" variant="muted" onClick={onClose}>
              Cancel
            </Button>
            <Button
              onClick={handleSubmit}
              disabled={selectedIds.size === 0 || submitting}
            >
              {submitting ? <Spinner size={14} /> : "Add"}
            </Button>
          </div>
        </div>
      </div>
    </Dialog>
  );
}
