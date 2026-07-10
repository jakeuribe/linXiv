// Run: node --experimental-strip-types --test src/lib/shortcuts.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { SHORTCUTS } from "./shortcuts.ts";

const keyEvent = (overrides: Partial<KeyboardEvent>): KeyboardEvent =>
  ({
    key: "",
    ctrlKey: false,
    metaKey: false,
    altKey: false,
    ...overrides,
  }) as KeyboardEvent;

function shortcut(id: string) {
  const s = SHORTCUTS.find((s) => s.id === id);
  assert.ok(s?.match, `expected shortcut "${id}" with a match predicate`);
  return s!.match!;
}

test("zoom-in matches Ctrl/Cmd with + or =", () => {
  const match = shortcut("zoom-in");
  assert.equal(match(keyEvent({ ctrlKey: true, key: "+" })), true);
  assert.equal(match(keyEvent({ ctrlKey: true, key: "=" })), true);
  assert.equal(match(keyEvent({ metaKey: true, key: "+" })), true);
  assert.equal(match(keyEvent({ metaKey: true, key: "=" })), true);
  assert.equal(match(keyEvent({ ctrlKey: true, altKey: true, key: "+" })), false);
  assert.equal(match(keyEvent({ key: "+" })), false);
  assert.equal(match(keyEvent({ ctrlKey: true, key: "-" })), false);
});

test("zoom-out matches Ctrl/Cmd with - or _", () => {
  const match = shortcut("zoom-out");
  assert.equal(match(keyEvent({ ctrlKey: true, key: "-" })), true);
  assert.equal(match(keyEvent({ ctrlKey: true, key: "_" })), true);
  assert.equal(match(keyEvent({ metaKey: true, key: "-" })), true);
  assert.equal(match(keyEvent({ metaKey: true, key: "_" })), true);
  assert.equal(match(keyEvent({ ctrlKey: true, altKey: true, key: "-" })), false);
  assert.equal(match(keyEvent({ key: "-" })), false);
  assert.equal(match(keyEvent({ ctrlKey: true, key: "0" })), false);
});

test("zoom-reset matches Ctrl/Cmd with 0", () => {
  const match = shortcut("zoom-reset");
  assert.equal(match(keyEvent({ ctrlKey: true, key: "0" })), true);
  assert.equal(match(keyEvent({ metaKey: true, key: "0" })), true);
  assert.equal(match(keyEvent({ ctrlKey: true, altKey: true, key: "0" })), false);
  assert.equal(match(keyEvent({ key: "0" })), false);
  assert.equal(match(keyEvent({ ctrlKey: true, key: "9" })), false);
});
