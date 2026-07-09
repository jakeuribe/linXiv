// Run: node --experimental-strip-types --test src/lib/submitShortcut.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { submitOnCtrlEnter } from "./submitShortcut.ts";
import type { KeyboardEvent } from "react";

const keyEvent = (overrides: Partial<KeyboardEvent>): KeyboardEvent =>
  ({
    key: "Enter",
    metaKey: false,
    ctrlKey: false,
    repeat: false,
    preventDefault: () => {},
    ...overrides,
  }) as KeyboardEvent;

test("non-Enter key is a no-op", () => {
  let calls = 0;
  const handler = submitOnCtrlEnter(() => calls++);
  handler(keyEvent({ key: "a", ctrlKey: true }));
  assert.equal(calls, 0);
});

test("Enter without a modifier is a no-op", () => {
  let calls = 0;
  const handler = submitOnCtrlEnter(() => calls++);
  handler(keyEvent({ key: "Enter" }));
  assert.equal(calls, 0);
});

test("Ctrl/Cmd-Enter prevents default and submits once", () => {
  for (const mod of ["ctrlKey", "metaKey"] as const) {
    let calls = 0;
    let prevented = false;
    const handler = submitOnCtrlEnter(() => calls++);
    handler(
      keyEvent({ [mod]: true, preventDefault: () => (prevented = true) }),
    );
    assert.equal(calls, 1);
    assert.equal(prevented, true);
  }
});

test("a repeated Ctrl/Cmd-Enter keydown is suppressed", () => {
  let calls = 0;
  const handler = submitOnCtrlEnter(() => calls++);
  handler(keyEvent({ ctrlKey: true, repeat: true }));
  assert.equal(calls, 0);
});
