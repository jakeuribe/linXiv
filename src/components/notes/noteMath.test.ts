// Run: node --experimental-strip-types --test src/components/notes/noteMath.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { extractMath, restoreMath } from "./noteMath.ts";

test("ordinary number survives alongside a math span round-trip", () => {
  const raw = "The result in 2023 was significant. See $E=mc^2$ for details.";
  const { text, math } = extractMath(raw);
  const restored = restoreMath(text, math);
  assert.equal(restored, raw);
  assert.match(restored, /2023/);
  assert.match(restored, /\$E=mc\^2\$/);
});

test("two math spans separated by numbers restore to the correct positions", () => {
  const raw = "Equation 1 is $x$ and equation 10 is $y$.";
  const { text, math } = extractMath(raw);
  const restored = restoreMath(text, math);
  assert.equal(restored, raw);
  assert.match(restored, /Equation 1 is \$x\$/);
  assert.match(restored, /equation 10 is \$y\$/);
});
