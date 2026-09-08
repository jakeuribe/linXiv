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
  ArxivSearchResponse,
  ArxivFetchResponse,
  OpenAlexSearchResponse,
  SavedPdf,
  MergeCandidates,
  BasicAuthorDetails,
  AuthorWithCount,
  AuthorWithPapers,
  AuthorPaperPreview,
  Status,
  Stats,
  DoiVersionCandidate,
  MergeReceipt,
  FullTextReceipt,
  PaperMembershipReceipt,
  BibtexImportReceipt,
  FilterField,
  FilterAction,
  TagWithCount,
  NewVersion,
  OrcidCandidate,
  ImportPreview,
  ImportPreviewResponse,
  ImportedProject,
  PaperImportResult,
  PapersListing,
  PaperVersionMeta,
  PaperVersionsResponse,
  DoiCandidates,
  FullTextPending,
  SavedSourceIds,
  DeletedPaperReceipt,
  RemovedFromProjects,
  OkReceipt,
  SavedPdfListing,
  DeletedPdf,
  BackupInfo,
  DeletedPaperDetails,
  TrashedProjectRow,
  RestoredPaper,
  EditorProjectSummary,
  ProjectsResponse,
  CreatedProject,
  BulkAddReceipt,
  TagsResponse,
  TagDetail,
  AuthorsResponse,
  AuthorMergeResponse,
  PaperMetadata,
  OpenAlexSaveResponse,
  DoiResolveResponse,
  DoiSaveResponse,
  NoteListResponse,
  NoteGetResponse,
  DeletedNote,
  AnnotationListResponse,
  CreatedAnnotation,
  ReadingStatusesResponse,
  ReadingStatusReceipt,
  EditorProjectsResponse,
  SearchHistoryResponse,
  VersionCheckResponse,
  NewVersionsResponse,
  FeedRulesResponse,
  OrcidBackfillResponse,
  HardDeletedPaper,
  RestoredProject,
  HardDeletedProject,
  NoteCreateBody,
  NoteUpdateBody,
  AnnotationCreateBody,
  AnnotationUpdateBody,
  AuthorMergeBody,
  AuthorUpdateBody,
  CreateEditorProjectBody,
  ProjectCreateBody,
  ProjectUpdateBody,
  ProjectAddPaperBody,
  ProjectAddPapersBulkBody,
  ProjectExportBody,
  PaperSavedBody,
  PaperMergeBody,
  UploadPdfBody,
  ImportPdfBody,
  ImportBibtexBody,
  ImportPreviewBody,
  ImportCommitBody,
  ArxivSearchBody,
  ArxivFetchBody,
  OpenAlexSearchBody,
  OpenAlexSaveBody,
  DoiResolveBody,
  DoiSaveBody,
  FeedDismissBody,
  FeedRuleCreateBody,
  StorageBackupBody,
  StorageRestoreBody,
  OrcidBackfillBody,
  VersionsCheckBody,
  VersionsAckBody,
  ReadingStatusPutBody,
  EnvPatchBody,
  SummaryRow,
  SharedProjectsListing,
  ReceivedListing,
  ImportedReceipt,
  UnpublishedReceipt,
  LeftReceipt,
  UnlinkedReceipt,
  PublishedReceipt,
  TicketMinted,
  MemberCode,
  InviteMinted,
  MembersListing,
  MemberRow,
  RoleChanged,
  RevokedReceipt,
  RekeyedReceipt,
  RemovedReceipt,
  SharedPdfSaved,
  SyncDirection,
  ShareSettings,
} from "./generated";

// `PUT /api/papers/sfk/{fk}` body — core names it RepairFields; the UI has
// always called it PaperRepairBody.
export type { RepairFields as PaperRepairBody } from "./generated";

// History (`/api/history`): change log, per-change diff, restore.
export type {
  ChangeRow,
  Timeline,
  PaperChange,
  EntryChange,
  FieldChange,
  HistoryDiff,
  RestoredToChange,
  RestoreBody,
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

// The Knowledge Graph's wire shapes are GENERATED from `linxiv_core::graph`
// (`GraphView` and friends in ./generated.ts); `npm run types:check` is the
// drift check.

// Core's `service::feed::FeedResponse` carries `entries` as untyped JSON blobs
// (the cached feed entries round-trip through the DB as stored JSON), so the
// generated type would say `JsonValue` where the app relies on this shape —
// hand-written until core types the cached entries.
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

// Request-side only: the search form's clause rows. route/search.rs takes them
// as an untyped `Vec<Map<String, Value>>`.
export interface Clause {
  operator: "AND" | "OR" | "AND NOT";
  field: "all" | "ti" | "au" | "abs";
  value: string;
  uid: string;
}
