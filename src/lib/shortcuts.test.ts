// Run: node --experimental-strip-types --test src/lib/shortcuts.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  SHORTCUTS,
  activeShortcutCombos,
  effectiveMatch,
  findConflict,
  hasBindableModifier,
  shortcutForCombo,
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

// --- Combos crossing a frame boundary ----------------------------------
// The Knowledge Graph runs in an iframe, and key events do not cross a frame
// boundary: while it has focus (which it takes on the first canvas click)
// useGlobalShortcuts sees nothing at all. The guest is handed the combo list
// instead and hands the matching keydowns back, so these guards pin the
// spelled-out `defaultCombos` to the `match` predicates they stand in for — a
// combo listed here that `match` rejects is a key the guest swallows and the
// host then ignores.

test("every dispatchable shortcut spells out the combos it fires on", () => {
  for (const s of SHORTCUTS) {
    if (!s.run) continue;
    assert.ok(
      s.defaultCombos?.length,
      `shortcut "${s.id}" is dispatchable but names no defaultCombos, so a ` +
        "focused iframe cannot know to forward it"
    );
  }
});

test("every default combo is one its own shortcut's match predicate accepts", () => {
  for (const s of SHORTCUTS) {
    for (const combo of s.defaultCombos ?? []) {
      // `shift: null` means "either", so both values have to land.
      const shifts = combo.shift === null ? [true, false] : [combo.shift];
      for (const shift of shifts) {
        const hit = shortcutForCombo({ ...combo, shift }, {});
        assert.equal(
          hit?.id,
          s.id,
          `${s.id} lists ${JSON.stringify({ ...combo, shift })}, which dispatches to ` +
            `${hit?.id ?? "nothing"}`
        );
      }
    }
  }
});

test("activeShortcutCombos covers Ctrl and Shift+Ctrl spellings of the zoom chords", () => {
  const combos = activeShortcutCombos({});
  // '+' is Shift+'=' on most layouts and the zoom matchers ignore Shift, so a
  // combo that pinned one value would miss half of how the chord is typed.
  for (const key of ["+", "=", "-", "_", "0"]) {
    const hit = combos.find((c) => c.key === key);
    assert.ok(hit, `no combo forwards Ctrl+${key}`);
    assert.equal(hit!.ctrl, true);
    assert.equal(hit!.alt, false);
    assert.equal(hit!.shift, null, `Ctrl+${key} must forward with Shift either way`);
  }
});

test("activeShortcutCombos forwards a rebound chord instead of its default", () => {
  const override = { ctrl: true, alt: true, shift: false, key: "k" };
  const combos = activeShortcutCombos({ "zoom-in": override });
  assert.deepEqual(
    combos.filter((c) => c.key === "k"),
    [{ ctrl: true, alt: true, shift: false, key: "k" }]
  );
  assert.equal(combos.some((c) => c.key === "+"), false, "the replaced default must not linger");
  // ...and the other shortcuts keep theirs.
  assert.ok(combos.some((c) => c.key === "0"));
});

test("shortcutForCombo dispatches a rebound chord and drops the chord it replaced", () => {
  const overrides = { "zoom-in": { ctrl: true, alt: true, shift: false, key: "k" } };
  assert.equal(shortcutForCombo({ ctrl: true, alt: true, shift: false, key: "k" }, overrides)?.id, "zoom-in");
  assert.equal(shortcutForCombo({ ctrl: true, alt: false, shift: false, key: "+" }, overrides), undefined);
});

test("shortcutForCombo ignores combos nothing is bound to, and form-scoped ones", () => {
  assert.equal(shortcutForCombo({ ctrl: true, alt: false, shift: false, key: "q" }, {}), undefined);
  assert.equal(shortcutForCombo({ ctrl: false, alt: false, shift: false, key: "0" }, {}), undefined);
  // `submit` matches Ctrl+Enter but has no `run` — dispatch is element-scoped,
  // so forwarding it from the iframe must not fire anything.
  assert.equal(shortcutForCombo({ ctrl: true, alt: false, shift: false, key: "Enter" }, {}), undefined);
});
