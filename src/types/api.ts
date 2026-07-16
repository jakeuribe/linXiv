import type { ThemeColors, ColorAlphas } from "../lib/theme";

export interface Stats {
  paper_count: number;
  tag_count: number;
  category_count: number;
  pdf_count: number;
  recent_papers: Paper[];
}

export interface Paper {
  source_id: string;
  source_fk: number;
  version: number;
  title: string;
  summary: string | null;
  authors: string | string[];
  published: string | null;
  updated: string | null;
  url: string | null;
  doi: string | null;
  category: string | null;
  categories?: string[];
  journal_ref: string | null;
  comment: string | null;
  tags: string[];
  has_pdf: boolean;
  pdf_path: string | null;
  source: string | null;
}

export interface Project {
  id: number;
  name: string;
  description: string;
  color_hex: string | null;
  project_tags: string[];
  source_ids: string[];
  status: string;
  paper_count?: number;
  /** Persisted share identity (uuid v4); null until first publish. */
  share_id?: string | null;
}

export interface Note {
  id: number;
  source_fk: number;
  project_id: number | null;
  title: string;
  content: string;
  created_at: string | null;
  updated_at: string | null;
}

// PDF highlight annotation. `anchor` is opaque JSON (see lib/pdfAnchor); `comment`
// is the written comment ("" = highlight-only).
export interface Annotation {
  id: number;
  source_fk: number;
  project_id: number | null;
  anchor: string;
  comment: string;
  created_at: string | null;
  updated_at: string | null;
}

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

export interface SearchResult {
  source_id: string;
  version: number;
  title: string;
  summary: string;
  authors: string[];
  published: string;
  paper_url: string;
  primary_category: string;
  entry_id: string;
}

export interface Settings {
  pdf_save_limit_mb: number;
  theme_overrides: Partial<ThemeColors>;
  theme_override_alphas: ColorAlphas;
  search_history_enabled?: boolean;
  search_history_max?: number;
  tex_rendering_enabled?: boolean;
  home_feed_url?: string;
  [key: string]: unknown;
}

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

export interface FeedFilterRule {
  rule_id: number;
  field: "TITLE" | "SUMMARY" | "AUTHOR";
  keywords: string;
  action: "DENY" | "ALLOW";
  enabled: boolean;
}

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

export interface Clause {
  operator: "AND" | "OR" | "AND NOT";
  field: "all" | "ti" | "au" | "abs";
  value: string;
  uid: string;
}

export interface Author {
  author_id: number;
  full_name: string | null;
  first_name: string | null;
  last_name: string | null;
  orcid: string | null;
  paper_count?: number;
}

export interface AuthorPaperPreview {
  paper_id: number;
  source_id: string;
  source_fk: number;
  version: number;
  title: string | null;
}

export interface AuthorDetail extends Author {
  paper_count: number;
  papers: AuthorPaperPreview[];
}
