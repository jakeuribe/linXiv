// The wire types the frontend consumes.
//
// Everything backed by a canonical serializer in linxiv-core is GENERATED into
// ./generated.ts (CONTEXT.md § Serializer Convention); this file only aliases
// those to the frontend's vocabulary and hand-writes the shapes that have no
// single Rust struct to generate from. Each hand-written one says why — if the
// reason goes away, delete it here and add a `#[derive(TS)]` there.
import type { ThemeColors, ColorAlphas } from "../lib/theme";
import type {
  PaperDetails,
  ProjectOut,
  NoteDetails,
  AnnotationDetails,
  SearchResultOut,
  AuthorWithCount,
  AuthorWithPapers,
  FilterRule,
} from "./generated";

export type {
  PaperDetails,
  ProjectOut,
  NoteDetails,
  AnnotationDetails,
  SearchResultOut,
  BasicAuthorDetails,
  AuthorWithCount,
  AuthorWithPapers,
  AuthorPaperPreview,
  Status,
  Stats,
  DoiVersionCandidate,
  FullTextReceipt,
  PaperMembershipReceipt,
  BibtexImportReceipt,
  FilterField,
  FilterAction,
} from "./generated";

// Frontend names for the generated serializers. The Rust name is the model,
// the alias is what the UI has always called it.
export type Paper = PaperDetails;
export type Project = ProjectOut;
export type Note = NoteDetails;
export type Annotation = AnnotationDetails;
export type SearchResult = SearchResultOut;
export type Author = AuthorWithCount;
export type AuthorDetail = AuthorWithPapers;
export type FeedFilterRule = FilterRule;

// --- Not generated ---------------------------------------------------------

// `GET /api/settings` returns `UserSettings::all()` — a free-form JSON object
// seeded from crates/core/assets/default_settings.json, plus the mailto env
// keys overlaid by route/settings.rs. There is no Rust struct, so the index
// signature is the honest type, not an escape hatch: the keys below are the
// ones the app actually reads.
export interface Settings {
  pdf_save_limit_mb: number;
  theme_overrides: Partial<ThemeColors>;
  theme_override_alphas: ColorAlphas;
  search_history_enabled?: boolean;
  search_history_max?: number;
  tex_rendering_enabled?: boolean;
  full_text_worker_enabled?: boolean;
  home_feed_url?: string;
  rss_cache_retention_days?: number;
  update_check_frequency?: string;
  /** Overlaid from the process env by route/settings.rs, never persisted here. */
  CROSSREF_MAILTO?: string;
  OPENALEX_MAILTO?: string;
  /** Self-hosted iroh relay override; empty keeps n0's public relays. Read once at app launch. */
  p2p_relay_url?: string;
  p2p_relay_auth_token?: string;
  /** If true, refuse to bind the p2p node at all rather than falling back to n0's public relay. */
  p2p_relay_only?: boolean;
  [key: string]: unknown;
}

// `GET /api/graph` builds its payload as a `serde_json::Value` in
// crates/core/src/graph.rs — no single Rust struct, so nothing to `#[derive(TS)]`
// and these stay hand-written. That also means nothing but a test can hold them
// to the payload, and left unchecked they had already drifted away from it: a
// paper node's `id` was declared `string` where graph.rs emits the bare
// SOURCE_FK integer, and eight of the fields the payload carries were missing
// altogether. src/lib/graphIframeAssets.test.ts now pins every field below
// against that file, in both directions.
//
// The nodes are a discriminated union on `type`, not one loose shape: only a
// paper node carries the paper metadata, only an author node carries the
// `author_id` its click handler navigates to, and a tag node is the
// id/label/type triple and nothing else.

/** `type: "paper"` — the latest version of one active root. */
export interface GraphPaperNode {
  /**
   * PAPER_ROOTS.SOURCE_FK, as a number — the `/library/:sfk` route param, and
   * the `source` of every edge. NOT the `source_id` below.
   */
  id: number;
  type: "paper";
  source_id: string;
  /**
   * PAPER.TITLE. Declared non-null because the column is NOT NULL; graph.rs
   * reads it as an `Option` defensively, and public/graph/graph.js's title
   * filter calls `.toLowerCase()` on it unguarded.
   */
  label: string;
  category: string | null;
  tags: string[];
  has_pdf: boolean;
  /**
   * PAPER_META.PUBLISHED, forwarded raw — so `0001-01-01` (chrono's `date.min`
   * sentinel for "no date") reaches the client as a date in year 1 rather than
   * as null, unlike every other serializer. graph.js folds it back.
   */
  published: string | null;
  url: string | null;
  doi: string | null;
  summary: string | null;
  /** Ids of the ACTIVE projects holding this paper, ascending. */
  project_ids: number[];
}

/** `type: "author"` — one AUTHOR row linked to at least one active paper. */
export interface GraphAuthorNode {
  /** `author::<AUTHOR_FK>`. */
  id: string;
  type: "author";
  /** AUTHOR.AUTHOR_FULL_NAME — the canonical spelling, so it follows renames. */
  label: string;
  /** AUTHOR_FK, the `/authors/:id` route param. */
  author_id: number;
}

/** `type: "tag"` — derived from the papers' own tags, not from the TAG table. */
export interface GraphTagNode {
  /** `tag::<lowercased label>`. */
  id: string;
  type: "tag";
  /**
   * Display casing of the first paper that used the tag — first in SQLite's
   * scan order, since PAPER_NODES_SQL has no ORDER BY, so it can change under a
   * plain Refresh and it need not be the canonical `TAG.TAG` label the Tags
   * index and TagPage show. public/graph/graph.js draws the chip with the
   * `/api/tags` spelling instead (`_canonicalTagLabel`) and falls back to this.
   */
  label: string;
}

export type GraphNode = GraphPaperNode | GraphAuthorNode | GraphTagNode;

/**
 * Always paper → author or paper → tag, so `source` is a paper node's numeric
 * id and `target` is a prefixed string one. There is no edge `type`.
 */
export interface GraphEdge {
  source: number;
  target: string;
}

export interface GraphData {
  nodes: GraphNode[];
  edges: GraphEdge[];
}

// `GET /api/graph/project-options` — the graph's Projects / Project Tags filter
// chips. Assembled by the same `json!` in crates/core/src/graph.rs and wrapped
// in `{projects: …}` by src-tauri/src/route/graph.rs.
//
// Every ACTIVE project, whether or not any paper `/api/graph` drew belongs to
// it — the two endpoints are answered independently. Both filter boxes match a
// paper through `GraphPaperNode.project_ids`, so public/graph/graph.js narrows
// what it OFFERS to the projects that appear there (and marks a hand-typed row
// that names only the others), the same split it makes for the Paper Tags
// dropdown against `/api/tags`. A consumer that shows this list raw is offering
// filters that can only empty the canvas.
export interface GraphProjectOption {
  id: number;
  name: string;
  /** Always set: graph.rs falls back to its own default when a project has none. */
  color: string;
  /** PROJECT_TO_TAG labels, ordered by label. */
  tags: string[];
}

export interface GraphProjectOptions {
  projects: GraphProjectOption[];
}

// sources/feed.rs::FeedEntry has a matching Rust struct, but the response is
// assembled inline in route/feed.rs (`title` + entries + saved ids), so only
// half of this pair could be generated.
export interface FeedEntry {
  title: string;
  link: string;
  authors: string[];
  summary: string;
  published: string;
  arxiv_id: string | null;
  version: number | null;
}

export interface FeedResponse {
  title: string;
  entries: FeedEntry[];
  saved_arxiv_ids: string[];
}

// `GET /api/papers/sfk/{fk}/versions` projects PaperDetailsAll into an inline
// `json!` in route/papers.rs — an ADR-0010 reach-past, not a serializer.
export interface PaperVersionSummary {
  version: number;
  published: string | null;
  updated: string | null;
  has_pdf: boolean;
}

export interface PaperVersionsResponse {
  source_id: string;
  latest_version: number;
  versions: PaperVersionSummary[];
}

// Request-side only: the search form's clause rows. route/search.rs takes them
// as an untyped `Vec<Map<String, Value>>`.
export interface Clause {
  operator: "AND" | "OR" | "AND NOT";
  field: "all" | "ti" | "au" | "abs";
  value: string;
  uid: string;
}
