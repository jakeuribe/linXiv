import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { updateProject } from "../../api/projects";
import { ColorSwatch } from "./ColorSwatch";
import { PRESET_COLORS } from "./constants";
import { TagInput, type TagInputHandle } from "./TagInput";
import { Button } from "../ui/button";
import { Dialog } from "../ui/dialog";
import { formSubmitOnCtrlEnter } from "../../lib/submitShortcut";
import { Input } from "../ui/input";
import { Textarea } from "../ui/input";
import { Spinner } from "../ui/spinner";
import { READING_LIST_TAG } from "../../lib/readingStatus";
import { invalidateProjectMutationQueries } from "../../lib/paperMutations";
import { errText } from "../../lib/errText";

interface EditProjectDialogProps {
  open: boolean;
  onClose: () => void;
  projectId: number;
  initialName: string;
  initialDescription: string;
  initialColor: string | null;
  initialTags: string[];
}

export function EditProjectDialog({
  open,
  onClose,
  projectId,
  initialName,
  initialDescription,
  initialColor,
  initialTags,
}: EditProjectDialogProps) {
  const queryClient = useQueryClient();
  const [name, setName] = useState(initialName);
  const [description, setDescription] = useState(initialDescription);
  const [color, setColor] = useState<string | null>(initialColor);
  const [tags, setTags] = useState<string[]>(initialTags);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const tagInputRef = useRef<TagInputHandle>(null);

  // Keep a mutable ref so the effect reads latest props without dependencies.
  const seedRef = useRef({ initialName, initialDescription, initialColor, initialTags });
  seedRef.current = { initialName, initialDescription, initialColor, initialTags };

  useEffect(() => {
    if (!open) return;
    setName(seedRef.current.initialName);
    setDescription(seedRef.current.initialDescription);
    setColor(seedRef.current.initialColor);
    setTags(seedRef.current.initialTags);
    setError(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (submitting) return;
    if (!name.trim()) return;
    setSubmitting(true);
    setError(null);
    try {
      // Read via imperative handle to capture any uncommitted draft text
      // (typed but not yet Enter'd — stale closure on tags state is unsafe here).
      const currentTags = tagInputRef.current?.getTagsWithDraft() ?? tags;
      const hasReadingListTag = currentTags.some(
        (t) => t.toLowerCase() === READING_LIST_TAG
      );
      const hadReadingListTag = initialTags.some(
        (t) => t.toLowerCase() === READING_LIST_TAG
      );
      if (hasReadingListTag && !hadReadingListTag) {
        setError("Cannot add reading-list tag to a non-reading-list project");
        setSubmitting(false);
        return;
      }
      await updateProject(projectId, {
        name: name.trim(),
        description: description.trim(),
        color_hex: color,
        project_tags: currentTags,
      });
      await invalidateProjectMutationQueries(queryClient);
      onClose();
    } catch (err) {
      setError(errText(err, "Failed to update project"));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Dialog open={open} onClose={onClose} title="Edit Project">
      <form onSubmit={handleSubmit} onKeyDown={formSubmitOnCtrlEnter} className="flex flex-col gap-4">
        <div className="flex flex-col gap-1.5">
          <label
            htmlFor="edit-proj-name"
            className="text-xs font-medium"
            style={{ color: "var(--color-muted)" }}
          >
            Name <span style={{ color: "var(--color-danger)" }}>*</span>
          </label>
          <Input
            id="edit-proj-name"
            value={name}
            onChange={(e) => setName(e.target.value)}
            required
            autoFocus
          />
        </div>

        <div className="flex flex-col gap-1.5">
          <label
            htmlFor="edit-proj-desc"
            className="text-xs font-medium"
            style={{ color: "var(--color-muted)" }}
          >
            Description
          </label>
          <Textarea
            id="edit-proj-desc"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
          />
        </div>

        <div className="flex flex-col gap-1.5">
          <span
            className="text-xs font-medium"
            style={{ color: "var(--color-muted)" }}
          >
            Color
          </span>
          <div className="flex items-center gap-2">
            {PRESET_COLORS.map((c) => (
              <ColorSwatch
                key={c}
                color={c}
                size={20}
                selected={color === c}
                onClick={() => setColor(color === c ? null : c)}
              />
            ))}
          </div>
        </div>

        <div className="flex flex-col gap-1.5">
          <label
            htmlFor="edit-proj-tags"
            className="text-xs font-medium"
            style={{ color: "var(--color-muted)" }}
          >
            Tags
          </label>
          <TagInput ref={tagInputRef} id="edit-proj-tags" value={tags} onChange={setTags} />
          <p className="text-xs" style={{ color: "var(--color-muted)" }}>
            Press Enter to add a tag. Backspace removes the last tag.
          </p>
        </div>

        {error && (
          <p className="text-xs" style={{ color: "var(--color-danger)" }}>
            {error}
          </p>
        )}

        <div className="flex justify-end gap-2 pt-1">
          <Button type="button" variant="muted" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" disabled={!name.trim() || submitting}>
            {submitting ? <Spinner size={14} /> : "Save"}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
