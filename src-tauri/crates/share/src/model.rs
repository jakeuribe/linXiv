//! The quarantined CRDT document model. Each struct derives autosurgeon's
//! `Reconcile`/`Hydrate` so it maps to/from an automerge document; `PartialEq`
//! backs the round-trip and merge-convergence assertions.
//!
//! Fields are projected from the canonical `linxiv_core::models` read views
//! (PaperDetails / NoteDetails / ProjectDetails). `color` widens i32→i64 and the
//! note timestamps are stored as ISO strings — automerge has no native i32/date
//! scalar that autosurgeon maps a `NaiveDateTime` onto without a custom impl.

use autosurgeon::{Hydrate, Reconcile};

#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate)]
pub struct SharedProject {
    pub share_id: String,
    pub name: String,
    pub description: String,
    pub color: Option<i64>,
    pub tags: Vec<String>,
    pub papers: Vec<SharedPaper>,
    pub notes: Vec<SharedNote>,
}

#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate)]
pub struct SharedPaper {
    pub source_id: String,
    pub version: i64,
    pub title: String,
    pub summary: String,
    pub authors: Vec<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate)]
pub struct SharedNote {
    pub id: i64,
    pub title: String,
    pub body: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

/// Lightweight listing view — hydrated counts without the full subgraph.
#[derive(Debug, Clone, PartialEq)]
pub struct SharedSummary {
    pub share_id: String,
    pub name: String,
    pub paper_count: usize,
    pub note_count: usize,
    pub tag_count: usize,
}
