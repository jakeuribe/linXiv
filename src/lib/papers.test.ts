import assert from "node:assert/strict";
import { test } from "node:test";

import type { Paper } from "../types/api";
import { isArxivId, isArxivPaper, landingUrl } from "./papers.ts";

const paper = (over: Partial<Paper>): Paper => ({ source_id: "", source: "", ...over }) as Paper;

test("isArxivPaper matches namespaced library ids, not bare-id regex", () => {
  assert.equal(isArxivPaper(paper({ source_id: "arxiv:2204.12985" })), true);
  assert.equal(isArxivPaper(paper({ source_id: "openalex:W123" })), false);
  assert.equal(isArxivPaper(paper({ source_id: "local:abc123" })), false);
  // Legacy rows read PROVIDER back as "arxiv" — the id prefix is the truth.
  assert.equal(isArxivPaper(paper({ source_id: "doi:10.1/x", source: "arxiv" })), false);
  // The bare-id helper stays for stripped search-result ids.
  assert.equal(isArxivId("2204.12985"), true);
  assert.equal(isArxivId("arxiv:2204.12985"), false);
});

test("landingUrl prefers the DOI resolver, falls back to the source url", () => {
  assert.equal(
    landingUrl(paper({ doi: "10.1/x", url: "https://a" })),
    "https://doi.org/10.1/x"
  );
  assert.equal(landingUrl(paper({ doi: null, url: "https://a" })), "https://a");
  assert.equal(landingUrl(paper({ doi: null, url: null })), null);
  // Free-text url field: never offer non-http(s) schemes to the OS opener.
  assert.equal(landingUrl(paper({ doi: null, url: "file:///etc/passwd" })), null);
});
