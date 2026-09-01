import { useEffect, useState } from "react";
import type { Paper } from "../../types/api";
import { createLecture } from "../../api/papers";
import { Dialog } from "../ui/dialog";
import { Input } from "../ui/input";
import { Button } from "../ui/button";
import { errText } from "../../lib/errText";

interface AddLectureDialogProps {
  open: boolean;
  onClose: () => void;
  onDone: (paper: Paper) => void;
}

export function AddLectureDialog({ open, onClose, onDone }: AddLectureDialogProps) {
  const [url, setUrl] = useState("");
  const [title, setTitle] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) {
      setUrl("");
      setTitle("");
      setError(null);
      setSaving(false);
    }
  }, [open]);

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    if (!url.trim() || !title.trim() || saving) return;
    setSaving(true);
    setError(null);
    try {
      onDone(await createLecture({ url: url.trim(), title: title.trim() }));
    } catch (err) {
      setError(errText(err, "Failed to add lecture"));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Dialog open={open} onClose={onClose} title="Add YouTube lecture">
      <form className="space-y-4" onSubmit={submit}>
        <p className="text-sm text-muted">
          linXiv stores the lecture link and your notes locally. The video streams directly from YouTube.
        </p>
        <label className="block space-y-1.5 text-sm text-text">
          <span>Video URL</span>
          <Input autoFocus value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://www.youtube.com/watch?v=…" disabled={saving} />
        </label>
        <label className="block space-y-1.5 text-sm text-text">
          <span>Lecture title</span>
          <Input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="Lecture title" disabled={saving} />
        </label>
        {error && <p className="text-sm text-danger">{error}</p>}
        <div className="flex justify-end gap-2">
          <Button type="button" variant="ghost" size="sm" onClick={onClose} disabled={saving}>Cancel</Button>
          <Button type="submit" variant="primary" size="sm" disabled={saving || !url.trim() || !title.trim()}>
            {saving ? "Adding…" : "Add lecture"}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
