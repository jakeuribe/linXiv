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
  [key: string]: unknown;
}

// `GET /api/graph` builds its payload as a `serde_json::Value` in
// crates/core/src/graph.rs — no struct to derive on.
export interface GraphData {
  nodes: GraphNode[];
  edges: GraphEdge[];
}

export interface GraphNode {
  id: string;
  label: string;
  type: string;
  tags?: string[];
  project_ids?: number[];
}

export interface GraphEdge {
  source: string;
  target: string;
  type?: string;
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
