// Pure (JSX-free) math-placeholder extraction for the note markdown preview, split
// out of NoteMarkdown.tsx so it can be unit-tested with `node --experimental-strip-types`
// (which strips type annotations but cannot parse the JSX in the .tsx).

// Matches, in priority order: fenced code blocks, inline code spans, display
// math $$...$$, inline math $...$. Code spans are passed through unchanged.
const MATH_OR_CODE_RE = /```[\s\S]*?```|`[^`\n]*`|\$\$[\s\S]+?\$\$|\$(?!\s)[^$\n]*?[^$\s]\$/g;

// Private Use Area sentinels (U+E000 open, U+E001 close) bracket a math[] index so the
// placeholder can't collide with ordinary digits in note text (e.g. a note that contains
// "3 apples" or "section 2"). PUA codepoints don't occur in normal text, so the restore
// regex only ever matches placeholders we inserted — a bare "\d+" token would clobber any
// real digit. Built via fromCharCode/RegExp so the source stays ASCII (literal PUA chars
// don't survive round-trips).
const MATH_OPEN = String.fromCharCode(0xe000);
const MATH_CLOSE = String.fromCharCode(0xe001);
const MATH_TOKEN_RE = new RegExp(MATH_OPEN + "(\\d+)" + MATH_CLOSE, "g");

// Pull $…$ / $$…$$ spans out of the raw markdown into placeholder tokens
// before react-markdown parses it.
export function extractMath(raw: string): { text: string; math: string[] } {
  const math: string[] = [];
  const text = raw.replace(MATH_OR_CODE_RE, (m) => {
    if (m[0] === "`") return m;
    const token = `${MATH_OPEN}${math.length}${MATH_CLOSE}`;
    math.push(m);
    return token;
  });
  return { text, math };
}

// Swap placeholder tokens in a rendered leaf string back for their original
// $…$ / $$…$$ source text.
export function restoreMath(text: string, math: string[]): string {
  if (math.length === 0) return text;
  return text.replace(MATH_TOKEN_RE, (_m, i: string) => math[Number(i)] ?? "");
}
