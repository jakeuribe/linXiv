//! pdf_metadata — PDF-first metadata extraction (D10 spike). pdfium reads
//! first-page text + the Info dict; scanners/heuristics are hand-rolled (no
//! `regex` dep). A malformed PDF degrades to an all-None record — see the D30
//! note in `extract_pdf_metadata` for the release/`abort` and segfault caveats.
//!
//! `resolve_pdf_metadata` is PDF-metadata-first: the PDF's own title/authors win
//! outright when both present. A text-scanned arXiv id/DOI is only a candidate
//! dedupe identity (page 1 can cite someone else's paper): with
//! `pdf_import_verify_identity_enabled` (default true) it is confirmed over the
//! network before being trusted — off, it is simply never adopted. Insufficient
//! PDF metadata enriches from arXiv/DOI/CrossRef (`enrich_external`, not gated),
//! else falls back to a partial record.
//!
//! Layout: `scan` (byte scanners), `extract` (pdfium FFI + discovery), `worker`
//! (subprocess crash boundary), `identity` (local sha256 id), `resolve` (orchestration).

mod extract;
mod identity;
mod resolve;
mod scan;
mod worker;

pub use resolve::resolve_pdf_metadata;
pub use worker::{extract_pdf_metadata_json, PDF_META_SUBCOMMAND};
