//! sources — upstream metadata/PDF providers (Rust port of `sources/`). Phase 3.

pub mod http;
pub mod arxiv;
pub mod openalex;
pub mod crossref;
pub mod doi_resolve;
pub mod pdf_metadata;
pub mod download;
pub mod arxiv_downloads;
pub mod fetch;
