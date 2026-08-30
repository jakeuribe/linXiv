import { useEffect, useState } from "react";
import { Download } from "lucide-react";
import { useUiStore, type ExportFormatKey } from "../../stores/ui";
import { exportProject, exportBibtex, exportObsidian } from "../../api/exportImport";
import { Button } from "../ui/button";
import { Dialog } from "../ui/dialog";
import { Spinner } from "../ui/spinner";
import { errText } from "../../lib/errText";

export function ExportDialog({
  open,
  onClose,
  projectId,
  projectName,
}: {
  open: boolean;
  onClose: () => void;
  projectId: number;
  projectName?: string;
}) {
  const exportMethods = useUiStore((s) => s.exportMethods);
  const [includePdfs, setIncludePdfs] = useState(false);
  const [busy, setBusy] = useState<ExportFormatKey | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (open) {
      setError(null);
      setIncludePdfs(false);
    }
  }, [open]);

  async function handleExport(format: ExportFormatKey) {
    setBusy(format);
    setError(null);
    try {
      if (format === "lxproj") {
        await exportProject(projectId, includePdfs, projectName);
      } else if (format === "bibtex") {
        await exportBibtex(projectId, projectName);
      } else if (format === "obsidian") {
        await exportObsidian(projectId, projectName);
      } else {
        format satisfies never;
      }
      onClose();
    } catch (e) {
      if (e instanceof Error && e.name === "AbortError") return; // file-picker cancelled — keep dialog open, show nothing
      setError(errText(e, "Export failed"));
    } finally {
      setBusy(null);
    }
  }

  const anyEnabled = Object.values(exportMethods).some(Boolean);

  return (
    <Dialog open={open} onClose={onClose} title="Export Project">
      <div className="flex flex-col gap-4">
        {exportMethods.lxproj && (
          <div className="flex flex-col gap-3">
            <label className="flex items-center gap-2 text-sm cursor-pointer select-none" style={{ color: "var(--color-text)" }}>
              <input
                type="checkbox"
                checked={includePdfs}
                onChange={(e) => setIncludePdfs(e.target.checked)}
                className="accent-[var(--color-accent)]"
              />
              Include PDFs in .lxproj archive
            </label>
            {(exportMethods.bibtex || exportMethods.obsidian) && (
              <p className="text-xs" style={{ color: "var(--color-muted)" }}>
                BibTeX and Obsidian exports include paper metadata only.
              </p>
            )}
          </div>
        )}

        {!anyEnabled && (
          <p className="text-sm" style={{ color: "var(--color-muted)" }}>
            No export formats are enabled. Enable them in Settings → Export Methods.
          </p>
        )}

        {error && (
          <p className="text-xs" style={{ color: "var(--color-danger)" }}>{error}</p>
        )}

        <div className="flex gap-2 justify-end pt-1 flex-wrap">
          <Button variant="muted" onClick={onClose}>Cancel</Button>
          {exportMethods.bibtex && (
            <Button variant="muted" onClick={() => handleExport("bibtex")} disabled={!!busy}>
              {busy === "bibtex" ? <Spinner size={14} /> : <><Download size={13} className="mr-1" />BibTeX</>}
            </Button>
          )}
          {exportMethods.obsidian && (
            <Button variant="muted" onClick={() => handleExport("obsidian")} disabled={!!busy}>
              {busy === "obsidian" ? <Spinner size={14} /> : <><Download size={13} className="mr-1" />Obsidian</>}
            </Button>
          )}
          {exportMethods.lxproj && (
            <Button onClick={() => handleExport("lxproj")} disabled={!!busy}>
              {busy === "lxproj" ? <Spinner size={14} /> : <><Download size={13} className="mr-1" />.lxproj</>}
            </Button>
          )}
        </div>
      </div>
    </Dialog>
  );
}
