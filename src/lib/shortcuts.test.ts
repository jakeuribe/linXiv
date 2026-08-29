// Run: node --experimental-strip-types --test src/lib/shortcuts.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  SHORTCUTS,
  effectiveMatch,
  findConflict,
  hasBindableModifier,
} from "./shortcuts.ts";

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

test("hasBindableModifier requires Ctrl, Cmd, or Alt", () => {
  assert.equal(hasBindableModifier(keyEvent({ ctrlKey: true, key: "a" })), true);
  assert.equal(hasBindableModifier(keyEvent({ metaKey: true, key: "a" })), true);
  assert.equal(hasBindableModifier(keyEvent({ altKey: true, key: "a" })), true);
  assert.equal(hasBindableModifier(keyEvent({ key: "a" })), false);
  assert.equal(
    hasBindableModifier({ ...keyEvent({ key: "A" }), shiftKey: true } as KeyboardEvent),
    false
  );
});

test("findConflict catches a rebind that collides with the submit chord", () => {
  const hit = findConflict("zoom-in", keyEvent({ ctrlKey: true, key: "Enter" }), {});
  assert.equal(hit?.id, "submit");
});

test("findConflict ignores the shortcut being rebound itself", () => {
  const hit = findConflict("zoom-reset", keyEvent({ ctrlKey: true, key: "0" }), {});
  assert.equal(hit, undefined);
});

test("effectiveMatch prefers a user override over the shortcut's default", () => {
  const s = SHORTCUTS.find((s) => s.id === "zoom-in")!;
  const override = { ctrl: true, alt: false, shift: false, key: "k" };
  const match = effectiveMatch(s, { "zoom-in": override })!;
  assert.equal(match(keyEvent({ ctrlKey: true, key: "k" })), true);
  assert.equal(match(keyEvent({ ctrlKey: true, key: "+" })), false);
});

test("effectiveMatch falls back to the shortcut's default with no override", () => {
  const s = SHORTCUTS.find((s) => s.id === "zoom-in")!;
  assert.equal(effectiveMatch(s, {}), s.match);
});
