import {
  Children,
  cloneElement,
  isValidElement,
  type JSXElementConstructor,
  type ReactNode,
} from "react";
import Markdown, { type Components } from "react-markdown";
import { MathText } from "../../lib/tex";
import { extractMath, restoreMath } from "./noteMath";

// Matches the shape of ReactElement["type"]: an intrinsic tag name or a component.
type LeafType = string | JSXElementConstructor<any>;

// True when withMath should stop recursing into an element: code/pre keep
// literal text, and MATH_TAGS components (matched by reference) already run
// their own withMath when they render.
export function shouldSkipMathWalk(
  element: { type: LeafType },
  mathComponents: Set<LeafType>,
): boolean {
  return element.type === "code" || element.type === "pre" || mathComponents.has(element.type);
}

// Walk react-markdown's rendered children and route every raw text string
// through MathText (after restoring placeholder tokens to source math) so
// `$…$` / `$$…$$` spans render as MathJax SVG. Recursion into inline
// elements (strong/em/a) reaches their text too.
function withMath(
  children: ReactNode,
  math: string[],
  mathComponents: Set<LeafType>,
  forceInline: boolean,
): ReactNode {
  return Children.map(children, (child) => {
    if (typeof child === "string") {
      return <MathText forceInline={forceInline}>{restoreMath(child, math)}</MathText>;
    }
    if (isValidElement(child)) {
      if (shouldSkipMathWalk(child, mathComponents)) return child;
      const kids = (child.props as { children?: ReactNode }).children;
      if (kids == null) return child;
      return cloneElement(
        child,
        undefined,
        withMath(kids, math, mathComponents, forceInline),
      );
    }
    return child;
  });
}

// Block containers whose text should become TeX-aware; inline elements
// nest inside them, and withMath recurses into those.
const MATH_TAGS = ["p", "li", "h1", "h2", "h3", "h4", "h5", "h6", "blockquote"] as const;

// Built per NoteMarkdown render so each tag renderer closes over that
// render's extracted math[]; MATH_COMPONENTS is rebuilt alongside it so its
// reference-equality check matches these specific component instances.
function createComponents(math: string[], forceInline: boolean): Components {
  const components: Components = Object.fromEntries(
    MATH_TAGS.map((tag) => [
      tag,
      ({ node: _node, children, ...rest }: { node?: unknown; children?: ReactNode }) => {
        const Tag = tag;
        return <Tag {...rest}>{withMath(children, math, mathComponents, forceInline)}</Tag>;
      },
    ]),
  );
  const mathComponents: Set<LeafType> = new Set(Object.values(components));
  return components;
}

// Minimal markdown preview: basic CommonMark (headings, lists, emphasis, code,
// blockquotes, links) plus inline/display math via the existing MathText. No
// remark-gfm, so tables/strikethrough aren't rendered.
// ponytail: CommonMark-only; add remark-gfm if notes need tables.
export function NoteMarkdown({
  content,
  className,
  forceInline = false,
}: {
  content: string;
  className?: string;
  /** Forces display ($$…$$) math to render inline instead of as a block —
   *  needed by callers (e.g. the line-clamped card preview) that can't host a
   *  display:block container. See MathText's forceInline for the mechanism. */
  forceInline?: boolean;
}) {
  const { text, math } = extractMath(content);
  return (
    <div className={"note-markdown" + (className ? " " + className : "")}>
      <Markdown components={createComponents(math, forceInline)}>{text}</Markdown>
    </div>
  );
}
