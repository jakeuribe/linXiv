import type { Note } from "../../types/api";
import { NoteMarkdown } from "./NoteMarkdown";

// Render a saved note's body the same way the editor's Preview tab does
// (markdown + math via NoteMarkdown), so what you see after saving matches
// what you saw while writing. forceInline (default, for the clamped card
// preview) keeps display math inline so a display:block container isn't
// promoted out of the line-clamp box; the full read page passes
// forceInline={false} so display equations render as proper centered blocks.
export function NoteBody({
  content,
  className,
  forceInline = true,
}: {
  content: string;
  className?: string;
  forceInline?: boolean;
}) {
  return <NoteMarkdown content={content} className={className} forceInline={forceInline} />;
}

// created_at and updated_at are equal at creation and diverge on PATCH. Compare
// parsed instants, not raw strings, so timestamp-formatting differences can't
// produce a false "edited" flag. Returns the timestamp to display + the flag.
export function noteEdited(note: Note): { date: string | null; edited: boolean } {
  const createdMs = note.created_at ? Date.parse(note.created_at) : NaN;
  const updatedMs = note.updated_at ? Date.parse(note.updated_at) : NaN;
  const edited =
    Number.isFinite(createdMs) &&
    Number.isFinite(updatedMs) &&
    updatedMs !== createdMs;
  return { date: edited ? note.updated_at : note.created_at, edited };
}
