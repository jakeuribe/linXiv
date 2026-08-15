// Run: node --experimental-transform-types --test src/stores/migrations.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  DEFAULT_EXPORT_METHODS,
  DEFAULT_SIDEBAR_PAGES,
  migrateTheme,
  migrateUi,
} from "./migrations.ts";
import { DEFAULT_ZOOM } from "../lib/zoom.ts";
import { DEFAULT_DENSITY } from "../lib/density.ts";

// ---------------------------------------------------------------------------
// theme store: v0 -> v3
// ---------------------------------------------------------------------------

/** What a v0 blob looked like: glassEffects, no alphas, no custom palettes. */
const themeV0 = () => ({
  preset: "Crimson",
  mode: "light",
  overrides: { bg: "#101010" },
  glassEffects: true,
});

test("theme v0 -> v3 keeps preset/mode/overrides and seeds the v2 fields", () => {
  assert.deepStrictEqual(migrateTheme(themeV0(), 0), {
    preset: "Crimson",
    mode: "light",
    overrides: { bg: "#101010" },
    overrideAlphas: {},
    customPalettes: [],
  });
});

test("theme v0 -> v3 drops every glass field", () => {
  const migrated = migrateTheme(
    {
      ...themeV0(),
      glassIntensity: 0.4,
      glassTintColor: "#ffffff",
      glassTintAlpha: 30,
    },
    0
  );
  for (const key of ["glassEffects", "glassIntensity", "glassTintColor", "glassTintAlpha"]) {
    assert.ok(!(key in migrated), `${key} should not survive`);
  }
});

/** v1 is the same shape as v0 minus glassEffects. */
const themeV1 = () => ({
  preset: "Navy",
  mode: "dark",
  overrides: { fg: "#eeeeee" },
  glassIntensity: 0.8,
});

test("theme v1 -> v3 keeps preset/mode/overrides and seeds the v2 fields", () => {
  assert.deepStrictEqual(migrateTheme(themeV1(), 1), {
    preset: "Navy",
    mode: "dark",
    overrides: { fg: "#eeeeee" },
    overrideAlphas: {},
    customPalettes: [],
  });
});

test("theme v0/v1 -> v3 resets overrideAlphas and customPalettes even if present", () => {
  // Documents current behaviour: these fields did not exist before v2, so a blob
  // claiming v0/v1 that carries them has them overwritten, not merged.
  const migrated = migrateTheme(
    {
      preset: "Navy",
      overrideAlphas: { bg: 50 },
      customPalettes: [{ name: "mine", preset: "Navy", overrides: {}, overrideAlphas: {} }],
    },
    1
  );
  assert.deepStrictEqual(migrated.overrideAlphas, {});
  assert.deepStrictEqual(migrated.customPalettes, []);
});

/** v2 introduced overrideAlphas + customPalettes; palettes still carried glass fields. */
const themeV2 = () => ({
  preset: "Forest",
  mode: "dark",
  overrides: { bg: "#0a0a0a" },
  overrideAlphas: { bg: 80 },
  customPalettes: [
    {
      name: "Mine",
      preset: "Forest",
      mode: "light",
      overrides: { bg: "#ffffff" },
      overrideAlphas: { bg: 100 },
      glassIntensity: 0.5,
      glassTintColor: "#123456",
      glassTintAlpha: 20,
    },
  ],
  glassIntensity: 0.2,
  glassTintColor: "#000000",
  glassTintAlpha: 10,
});

test("theme v2 -> v3 preserves alphas and palettes while stripping glass fields", () => {
  assert.deepStrictEqual(migrateTheme(themeV2(), 2), {
    preset: "Forest",
    mode: "dark",
    overrides: { bg: "#0a0a0a" },
    overrideAlphas: { bg: 80 },
    customPalettes: [
      {
        name: "Mine",
        preset: "Forest",
        mode: "light",
        overrides: { bg: "#ffffff" },
        overrideAlphas: { bg: 100 },
      },
    ],
  });
});

test("theme v2 -> v3 does not mutate the stored blob", () => {
  const stored = themeV2();
  const migrated = migrateTheme(stored, 2);
  assert.notEqual(migrated, stored as unknown, "returns a copy, not the stored object");
  assert.deepStrictEqual(stored, themeV2());
});

test("theme v2 -> v3 tolerates a non-array customPalettes", () => {
  assert.deepStrictEqual(migrateTheme({ preset: "Navy", customPalettes: null }, 2), {
    preset: "Navy",
    customPalettes: null,
  });
});

test("theme migrate handles undefined / null / empty blobs at every version", () => {
  const seeded = { overrideAlphas: {}, customPalettes: [] };
  for (const blob of [undefined, null, {}]) {
    assert.deepStrictEqual(migrateTheme(blob, 0), seeded);
    assert.deepStrictEqual(migrateTheme(blob, 1), seeded);
    assert.deepStrictEqual(migrateTheme(blob, 2), {});
    assert.deepStrictEqual(migrateTheme(blob, 3), {});
  }
});

test("theme migrate leaves a current/future version untouched", () => {
  const stored = { preset: "Navy", mode: "dark", overrides: {}, overrideAlphas: {}, customPalettes: [] };
  for (const version of [3, 4, 99]) {
    const migrated = migrateTheme(stored, version);
    assert.deepStrictEqual(migrated, stored);
    assert.notEqual(migrated, stored, "returns a copy, not the stored object");
  }
});

test("theme migrate keeps unknown keys it does not know about", () => {
  const migrated = migrateTheme({ preset: "Navy", somethingNew: 42 }, 2);
  assert.equal(migrated.somethingNew, 42);
});

// ---------------------------------------------------------------------------
// ui store: v1 -> v7
// ---------------------------------------------------------------------------

// Shared across the tests below without defensive copying: migrateUi copies on
// entry, so no call can disturb it for the next one.
const fullUi = {
  sidebarCollapsed: true,
  sidebarPages: { graph: false, search: true, doi: true, tags: true, notes: true },
  exportMethods: { lxproj: false, bibtex: true, obsidian: true },
  zoom: 1.5,
  density: "compact" as const,
  hideSingleAuthors: true,
};

test("ui v0 -> v7 fills every field from defaults", () => {
  assert.deepStrictEqual(migrateUi({ sidebarCollapsed: true }, 0), {
    sidebarCollapsed: true,
    sidebarPages: DEFAULT_SIDEBAR_PAGES,
    exportMethods: DEFAULT_EXPORT_METHODS,
    zoom: DEFAULT_ZOOM,
    hideSingleAuthors: false,
    density: DEFAULT_DENSITY,
  });
});

test("ui v1 -> v7 backfills exportMethods/zoom/hideSingleAuthors/density", () => {
  const migrated = migrateUi(
    { sidebarCollapsed: true, sidebarPages: { graph: false, search: true, doi: true } },
    1
  );
  assert.deepStrictEqual(migrated, {
    sidebarCollapsed: true,
    // user's `graph: false` survives; shared/reading/tags/notes come from defaults
    sidebarPages: { ...DEFAULT_SIDEBAR_PAGES, graph: false },
    exportMethods: DEFAULT_EXPORT_METHODS,
    zoom: DEFAULT_ZOOM,
    hideSingleAuthors: false,
    density: DEFAULT_DENSITY,
  });
});

test("ui v2 -> v7 keeps the user's exportMethods choices", () => {
  const migrated = migrateUi(
    { exportMethods: { lxproj: false, bibtex: true, obsidian: false }, sidebarPages: { doi: false } },
    2
  );
  assert.deepStrictEqual(migrated, {
    exportMethods: { lxproj: false, bibtex: true, obsidian: false },
    sidebarPages: { ...DEFAULT_SIDEBAR_PAGES, doi: false },
    zoom: DEFAULT_ZOOM,
    hideSingleAuthors: false,
    density: DEFAULT_DENSITY,
  });
});

test("ui v3 -> v7 keeps the saved zoom and seeds the v4/v5 fields", () => {
  // A real v3 blob has zoom but neither hideSingleAuthors nor density yet.
  const migrated = migrateUi(
    {
      sidebarCollapsed: true,
      sidebarPages: { ...DEFAULT_SIDEBAR_PAGES, graph: false },
      exportMethods: { lxproj: false, bibtex: true, obsidian: true },
      zoom: 1.5,
    },
    3
  );
  assert.deepStrictEqual(migrated, {
    sidebarCollapsed: true,
    sidebarPages: { ...DEFAULT_SIDEBAR_PAGES, graph: false },
    exportMethods: { lxproj: false, bibtex: true, obsidian: true },
    zoom: 1.5,
    hideSingleAuthors: false,
    density: DEFAULT_DENSITY,
  });
});

test("ui v1/v2 -> v7 resets a saved zoom to the default (zoom is a v3 field)", () => {
  // Documents current behaviour: a pre-v3 blob carrying `zoom` loses it.
  assert.equal(migrateUi(fullUi, 1).zoom, DEFAULT_ZOOM);
  assert.equal(migrateUi(fullUi, 2).zoom, DEFAULT_ZOOM);
});

test("ui v4 -> v7 keeps hideSingleAuthors and backfills density", () => {
  const migrated = migrateUi(fullUi, 4);
  assert.equal(migrated.hideSingleAuthors, true);
  assert.equal(migrated.zoom, 1.5);
  assert.equal(migrated.density, DEFAULT_DENSITY, "density is a v5 field, so it resets");
});

test("ui v5 -> v7 keeps density and backfills the shared page key", () => {
  const migrated = migrateUi(fullUi, 5);
  assert.equal(migrated.density, "compact");
  assert.deepStrictEqual(migrated.sidebarPages, {
    ...DEFAULT_SIDEBAR_PAGES,
    graph: false,
    tags: true,
    notes: true,
  });
});

test("ui v6 -> v7 backfills only the reading page key", () => {
  const migrated = migrateUi(
    { sidebarPages: { ...DEFAULT_SIDEBAR_PAGES, shared: false }, zoom: 0.8, density: "compact" },
    6
  );
  assert.deepStrictEqual(migrated, {
    sidebarPages: { ...DEFAULT_SIDEBAR_PAGES, shared: false, reading: true },
    zoom: 0.8,
    density: "compact",
  });
});

test("ui migrate handles undefined / null / empty blobs at every version", () => {
  const backfilled = {
    sidebarPages: DEFAULT_SIDEBAR_PAGES,
    exportMethods: DEFAULT_EXPORT_METHODS,
    zoom: DEFAULT_ZOOM,
    hideSingleAuthors: false,
    density: DEFAULT_DENSITY,
  };
  // The same blob is reused across all three calls: migrateUi copies on entry,
  // so no call's backfill can leak into the next assertion.
  for (const blob of [undefined, null, {}]) {
    assert.deepStrictEqual(migrateUi(blob, 0), backfilled);
    assert.deepStrictEqual(migrateUi(blob, 1), backfilled);
    assert.deepStrictEqual(migrateUi(blob, 6), { sidebarPages: DEFAULT_SIDEBAR_PAGES });
  }
});

test("ui migrate leaves a current/future version untouched", () => {
  for (const version of [7, 8, 99]) {
    assert.deepStrictEqual(migrateUi(fullUi, version), fullUi);
  }
});

test("ui migrate keeps unknown keys it does not know about", () => {
  const migrated = migrateUi({ somethingNew: 42 }, 6) as Record<string, unknown>;
  assert.equal(migrated.somethingNew, 42);
});

test("ui migrate does not mutate the persisted object", () => {
  const stored = { sidebarPages: { graph: false } };
  const migrated = migrateUi(stored, 6);
  assert.notEqual(migrated, stored as unknown, "returns a copy, not the stored object");
  assert.deepStrictEqual(stored, { sidebarPages: { graph: false } });
});

test("ui migrate tolerates a non-object sidebarPages", () => {
  assert.deepStrictEqual(migrateUi({ sidebarPages: null }, 6).sidebarPages, DEFAULT_SIDEBAR_PAGES);
});
