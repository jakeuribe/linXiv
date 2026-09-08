//! Fixtures shared by the crate's in-module unit tests. `#[cfg(test)]`-only, so
//! `tests/common/` (a separate compilation unit limited to the public API)
//! keeps its own four-line copy of [`db`]. Only genuinely identical fixtures
//! live here.

use crate::models::PaperMetadata;
use crate::storage::{db::open_in_memory, init_db};
use chrono::NaiveDate;
use rusqlite::Connection;

/// A fresh in-memory DB with the schema applied.
pub fn db() -> Connection {
    let conn = open_in_memory().unwrap();
    init_db(&conn).unwrap();
    conn
}

/// A minimal arXiv `PaperMetadata` — everything optional left unset, so a test
/// asserting on one field is not reading around a pile of unrelated fixture data.
pub fn meta(source_id: &str, version: i64) -> PaperMetadata {
    PaperMetadata {
        source_id: source_id.into(),
        version,
        title: format!("Title of {source_id} v{version}"),
        authors: vec!["Alice".into()],
        published: NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
        updated: None,
        summary: "s".into(),
        category: Some("cs.LG".into()),
        categories: Some(vec!["cs.LG".into()]),
        doi: None,
        journal_ref: None,
        comment: None,
        url: None,
        tags: None,
        source: Some("arxiv".into()),
        author_orcids: None,
    }
}
