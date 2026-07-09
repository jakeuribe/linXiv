import type { KeyboardEvent } from "react";

// Ctrl-Enter (Windows/Linux) or Cmd-Enter (macOS) — the conventional
// "submit this form/dialog" chord.
const isSubmitChord = (e: KeyboardEvent) => {
  if (e.repeat) return false;
  return (e.metaKey || e.ctrlKey) && !e.altKey && e.key === "Enter";
};

/** onKeyDown for a button-driven save surface (dialog/editor with no <form>). */
export function submitOnCtrlEnter(onSubmit: () => void) {
  return (e: KeyboardEvent) => {
    if (!isSubmitChord(e)) return;
    e.preventDefault(); // in a textarea, don't also insert a newline
    onSubmit();
  };
}

/** onKeyDown for a <form>: fires native submit so onSubmit + validation run. */
export function formSubmitOnCtrlEnter(e: KeyboardEvent<HTMLFormElement>) {
  if (!isSubmitChord(e)) return;
  e.preventDefault();
  e.currentTarget.requestSubmit();
}
