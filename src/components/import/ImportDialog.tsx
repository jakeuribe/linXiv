import { useRef, useState, useEffect } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Upload } from "lucide-react";
import { useImportJobsStore } from "../../stores/importJobs";
import {
  importBibtex,
  importPdf,
  commitImport,
  previewImport,
  type ImportPreview,
} from "../../api/exportImport";
import { Button } from "../ui/button";
import { Dialog } from "../ui/dialog";
import { submitOnCtrlEnter } from "../../lib/submitShortcut";


// ── Types ─────────────────────────────────────────────────────────────────────

type FileKind = "pdf" | "bibtex" | "lxproj" | "unknown";
type KnownFileKind = Exclude<FileKind, "unknown">;
let _uid = 0;
function nextUid() { return ++_uid; }

interface QueueEntry {
  uid: number;
  file: File;
  kind: KnownFileKind;
  preview?: ImportPreview;
  onConflict: "merge" | "overwrite";
}

function detectKind(f: File): FileKind {
  const n = f.name.toLowerCase();
  if (n.endsWith(".pdf")) return "pdf";
  if (n.endsWith(".bib")) return "bibtex";
  if (n.endsWith(".lxproj")) return "lxproj";
  return "unknown";
}

// ── Sub-components ────────────────────────────────────────────────────────────

function LxprojPreview({
  entry,
  onChange,
}: {
  entry: QueueEntry;
  onChange: (patch: Partial<QueueEntry>) => void;
}) {
  if (!entry.preview) return null;
  const p = entry.preview;
  return (
    <div
      className="ml-5 mt-1 mb-1 rounded border border-border p-2 text-xs flex flex-col gap-1"
      style={{ backgroundColor: "var(--color-bg)" }}
    >
      <p className="font-medium" style={{ color: "var(--color-text)" }}>{p.project_name}</p>
      {p.description && <p style={{ color: "var(--color-muted)" }}>{p.description}</p>}
      <p style={{ color: "var(--color-muted)" }}>
        {p.paper_count} paper{p.paper_count !== 1 ? "s" : ""} · {p.note_count} note{p.note_count !== 1 ? "s" : ""}
        {p.has_pdfs ? " · includes PDFs" : ""}
      </p>
      <div className="flex items-center gap-3 mt-0.5">
        <span style={{ color: "var(--color-muted)" }}>On conflict:</span>
        {(["merge", "overwrite"] as const).map((v) => (
          <label key={v} className="flex items-center gap-1 cursor-pointer capitalize" style={{ color: "var(--color-text)" }}>
            <input
              type="radio"
              name={`conflict-${entry.uid}`}
              value={v}
              checked={entry.onConflict === v}
              onChange={() => onChange({ onConflict: v })}
              className="accent-[var(--color-accent)]"
            />
            {v}
          </label>
        ))}
      </div>
    </div>
  );
}

// ── Main component ────────────────────────────────────────────────────────────

export interface ImportDialogProps {
  open: boolean;
  onClose: () => void;
  /** When set, PDFs and BibTeX entries are linked to this project after import. */
  projectId?: number;
  /** Called after all queued files finish processing. newProjectIds contains
   *  any project IDs created by .lxproj imports. */
  onDone: (newProjectIds: number[]) => void;
}

export function ImportDialog({ open, onClose, projectId, onDone }: ImportDialogProps) {
  const fileRef = useRef<HTMLInputElement>(null);
  const [queue, setQueue] = useState<QueueEntry[]>([]);
  const [rejectedFilenames, setRejectedFilenames] = useState<string[]>([]);
  const addJobs = useImportJobsStore((s) => s.addJobs);
  const updateStoreJob = useImportJobsStore((s) => s.updateJob);
  const queryClient = useQueryClient();
  // Keep onDone stable across the async loop even if parent re-renders or unmounts.
  const onDoneRef = useRef(onDone);
  useEffect(() => { onDoneRef.current = onDone; }, [onDone]);

  function updateEntry(uid: number, patch: Partial<QueueEntry>) {
    setQueue((prev) => prev.map((e) => (e.uid === uid ? { ...e, ...patch } : e)));
  }

  async function handleFilePick(e: React.ChangeEvent<HTMLInputElement>) {
    const all = Array.from(e.target.files ?? []).map((f) => ({ f, kind: detectKind(f) }));
    e.target.value = "";
    if (!all.length) return; // picker cancelled — don't clear the existing rejection warning
    const withKind = all.filter((x): x is { f: File; kind: KnownFileKind } => x.kind !== "unknown");
    const rejected = all.filter(({ kind }) => kind === "unknown").map(({ f }) => f.name);
    setRejectedFilenames(rejected); // per-batch: clears on next non-empty pick

    const newEntries: QueueEntry[] = withKind.map(({ f, kind }) => ({
      uid: nextUid(),
      file: f,
      kind,
      onConflict: "merge",
    }));
    setQueue((prev) => [...prev, ...newEntries]);

    // Auto-fetch previews for .lxproj files
    for (const entry of newEntries) {
      if (entry.kind !== "lxproj") continue;
      try {
        const preview = await previewImport(entry.file);
        setQueue((prev) =>
          prev.map((e) => (e.uid === entry.uid ? { ...e, preview } : e))
        );
      } catch {
        // preview is optional — import can still proceed
      }
    }
  }

  async function handleImport() {
    const toProcess = [...queue];
    if (!toProcess.length) return;

    // Register jobs in the global store so the sidebar can track them,
    // then close the dialog immediately — upload continues in the background.
    // Append to any existing jobs so concurrent batches don't clobber each other.
    addJobs(toProcess.map((e) => ({ uid: e.uid, filename: e.file.name })));
    reset();
    onClose();

    // Imports that produce papers must bust every cache that reads paper data.
    // Keys mirror PaperDetailPage's post-edit invalidation set.
    const invalidatePaperCaches = () => {
      // ["paper"] is a prefix that covers ["paper","sfk",...] and ["paper","versions",...].
      queryClient.invalidateQueries({ queryKey: ["paper"] });
      queryClient.invalidateQueries({ queryKey: ["papers"] });
      queryClient.invalidateQueries({ queryKey: ["stats"] });
      // New papers may introduce new tags, and pinning to a project changes the
      // project's paper count — bust both so the sidebar/index views refresh.
      queryClient.invalidateQueries({ queryKey: ["tags"] });
      queryClient.invalidateQueries({ queryKey: ["tag"] });
      if (projectId !== undefined) {
        queryClient.invalidateQueries({ queryKey: ["projects"] });
        // ProjectDetailPage keys the per-project query with String(id) from useParams.
        queryClient.invalidateQueries({ queryKey: ["project", String(projectId)] });
      }
    };

    const newProjectIds: number[] = [];
    // Sequential intentionally: avoids saturating the backend with concurrent uploads.
    for (const entry of toProcess) {
      try {
        const { file, kind, onConflict } = entry;
        let result = "";
        if (kind === "pdf") {
          const r = await importPdf(file, projectId);
          result = `Saved "${r.title || file.name}"`;
          invalidatePaperCaches();
        } else if (kind === "bibtex") {
          const r = await importBibtex(file, projectId);
          result = `${r.saved_count} paper${r.saved_count !== 1 ? "s" : ""} saved`;
          invalidatePaperCaches();
        } else if (kind === "lxproj") {
          const r = await commitImport(file, onConflict);
          newProjectIds.push(r.project_id);
          result = `Project imported`;
        } else {
          const _exhaustive: never = kind;
          throw new Error(`Unhandled file kind: ${_exhaustive}`);
        }
        updateStoreJob(entry.uid, { status: "done", result });
      } catch (err) {
        updateStoreJob(entry.uid, {
          status: "error",
          error: err instanceof Error ? err.message : String(err),
        });
      }
    }

    onDoneRef.current(newProjectIds);
  }

  function reset() {
    setQueue([]);
    setRejectedFilenames([]);
  }

  const queued = queue.length;

  return (
    <Dialog
      open={open}
      onClose={() => { reset(); onClose(); }}
      title="Import"
    >
      <div className="flex flex-col gap-4" onKeyDown={submitOnCtrlEnter(handleImport)}>
        {/* File picker */}
        <div>
          <input
            ref={fileRef}
            type="file"
            accept=".pdf,.bib,.lxproj"
            multiple
            onChange={handleFilePick}
            className="hidden"
          />
          <Button variant="muted" size="sm" onClick={() => fileRef.current?.click()}>
            <Upload size={13} className="mr-1.5" />
            Add files…
          </Button>
          <p className="text-xs mt-1" style={{ color: "var(--color-muted)" }}>
            Accepts .pdf · .bib (BibTeX) · .lxproj — multiple files supported
            {projectId ? " · files linked to this project" : ""}
          </p>
        </div>

        {/* Unsupported file warning */}
        {rejectedFilenames.length > 0 && (
          <p className="text-xs" style={{ color: "var(--color-danger)" }}>
            Unsupported format, skipped: {rejectedFilenames.join(", ")}
          </p>
        )}

        {/* Queue */}
        {queue.length > 0 && (
          <div
            className="rounded-md border border-border overflow-y-auto"
            style={{ maxHeight: 300, backgroundColor: "var(--color-bg)" }}
          >
            {queue.map((entry) => (
              <div key={entry.uid} className="border-b border-border last:border-0">
                <div className="flex items-center gap-2 px-3 py-2 text-sm">
                  <span className="w-3.5 shrink-0 inline-block text-center font-mono" style={{ color: "var(--color-muted)" }}>○</span>
                  <span className="flex-1 truncate" style={{ color: "var(--color-text)" }}>
                    {entry.file.name}
                  </span>
                  <span className="text-xs shrink-0" style={{ color: "var(--color-muted)" }}>
                    {entry.kind === "pdf" ? "PDF" : entry.kind === "bibtex" ? "BibTeX" : ".lxproj"}
                  </span>
                  <button
                    type="button"
                    className="text-sm shrink-0 leading-none"
                    style={{ color: "var(--color-muted)" }}
                    onClick={() => setQueue((prev) => prev.filter((e) => e.uid !== entry.uid))}
                  >
                    ×
                  </button>
                </div>
                {entry.kind === "lxproj" && (
                  <div className="px-3 pb-2">
                    <LxprojPreview entry={entry} onChange={(patch) => updateEntry(entry.uid, patch)} />
                  </div>
                )}
              </div>
            ))}
          </div>
        )}

        {/* Footer */}
        <div className="flex items-center justify-between pt-1">
          <span className="text-xs" style={{ color: "var(--color-muted)" }}>
            {queued > 0 ? `${queued} file${queued !== 1 ? "s" : ""} queued` : ""}
          </span>
          <div className="flex gap-2">
            <Button variant="muted" onClick={() => { reset(); onClose(); }}>
              Cancel
            </Button>
            <Button onClick={handleImport} disabled={queued === 0}>
              {`Import${queued > 0 ? ` (${queued})` : ""}`}
            </Button>
          </div>
        </div>
      </div>
    </Dialog>
  );
}
