import type { Note } from "../../types/api";
import { MathText } from "../../lib/tex";

// Render a note's body as TeX-aware text. The whole string goes through one
// MathText so a `$$…$$` display block can span lines; whitespace-pre-wrap keeps
// the newlines. (Tradeoff: two stray `$$` on different lines could merge into one
// span, but `$$`-as-currency is essentially never written, whereas multi-line
// display math is common.) forceInline (default, for the clamped card preview)
// keeps display math inline so a display:block container isn't promoted out of
// the line-clamp box; the full read page passes forceInline={false} so display
// equations render as proper centered blocks.
export function NoteBody({
  content,
  className,
  forceInline = true,
}: {
  content: string;
  className?: string;
  forceInline?: boolean;
}) {
  return (
    <div className={"whitespace-pre-wrap" + (className ? " " + className : "")}>
      <MathText forceInline={forceInline}>{content}</MathText>
    </div>
  );
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
