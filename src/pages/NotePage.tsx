import { useParams, useNavigate } from "react-router";
import { useQuery } from "@tanstack/react-query";
import { getNote } from "../api/notes";
import { getProject } from "../api/projects";
import { Spinner } from "../components/ui/spinner";
import { Button } from "../components/ui/button";
import { Badge } from "../components/ui/badge";
import { formatDate } from "../lib/date";
import { MathText } from "../lib/tex";
import { NoteBody, noteEdited } from "../components/notes/NoteBody";

// Dedicated read page for a single note. The note cards on a paper clamp content
// to a few lines; this is where a long note is read in full without dropping
// into the editor.
export default function NotePage() {
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();
  // Digit-only so a hand-typed "/notes/1e3" can't coerce to a different note id
  // than the URL names (Number("1e3") === 1000); mirrors the backend's path_i64.
  const noteId = id && /^\d+$/.test(id) ? Number(id) : NaN;

  const { data, isLoading, error } = useQuery({
    queryKey: ["note", noteId],
    queryFn: () => getNote(noteId),
    enabled: Number.isFinite(noteId),
  });

  // Resolve the scope to the real project name so the page agrees with the card
  // that linked here; falls back to a neutral label if the project is gone.
  const projectId = data?.note.project_id ?? null;
  const { data: scopeProject } = useQuery({
    // String key to match how the rest of the app caches projects (the id from
    // useParams elsewhere is a string), so an edit there invalidates this entry.
    queryKey: ["project", String(projectId)],
    queryFn: () => getProject(projectId!),
    enabled: projectId != null,
  });

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-full">
        <Spinner size={28} />
      </div>
    );
  }

  if (error || !data) {
    return (
      <div className="flex flex-col items-center justify-center gap-3 h-full">
        <p className="text-sm" style={{ color: "var(--color-danger)" }}>
          This note could not be found.
        </p>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => (window.history.length > 1 ? navigate(-1) : navigate("/library"))}
        >
          ← Back
        </Button>
      </div>
    );
  }

  const note = data.note;
  const { date: stamp, edited: isEdited } = noteEdited(note);
  const scopeLabel =
    note.project_id == null ? "Global" : scopeProject?.name ?? "Project-scoped";

  return (
    <div className="max-w-3xl mx-auto px-6 py-8 space-y-6">
      <div className="flex items-center justify-between gap-4">
        <Button
          variant="ghost"
          size="sm"
          onClick={() => (window.history.length > 1 ? navigate(-1) : navigate(`/library/${note.source_fk}`))}
        >
          ← Back
        </Button>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => navigate(`/library/${note.source_fk}`)}
        >
          Open paper →
        </Button>
      </div>

      <div className="space-y-2">
        <h1 className="font-display text-[27px] font-semibold leading-tight tracking-[-0.015em] text-text">
          <MathText forceInline>{note.title || "Untitled note"}</MathText>
        </h1>
        <div className="flex items-center gap-2 text-xs">
          <Badge color={scopeProject?.color_hex ?? undefined}>{scopeLabel}</Badge>
          <span style={{ color: "var(--color-muted)" }}>
            {formatDate(stamp)}
            {isEdited && " · edited"}
          </span>
        </div>
      </div>

      {note.content ? (
        <NoteBody
          content={note.content}
          forceInline={false}
          className="text-text text-[15px] leading-relaxed"
        />
      ) : (
        <p className="text-sm" style={{ color: "var(--color-muted)" }}>
          This note has no content.
        </p>
      )}
    </div>
  );
}
