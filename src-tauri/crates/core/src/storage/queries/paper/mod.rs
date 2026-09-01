//! Paper storage queries, split by concern. The public API is flat
//! (`storage::queries::paper::X`) via the re-exports below.

mod fts;
mod merge;
mod pdf;
mod read;
mod roots;
mod trash;
mod write;

pub use fts::{full_text_backfill_candidates, full_text_backfill_count, set_full_text};
pub use merge::{merge_paper_roots, merge_plan, MergePlan, MergeStats, VersionAction};
pub use pdf::{mark_pdf_saved, pdf_path_for_version, set_has_pdf, set_pdf_path};
pub(in crate::storage::queries) use read::row_to_paper;
pub use read::{
    existing_source_ids, find_doi_version_candidates, get_all_versions, get_categories, get_paper,
    get_paper_by_id, get_papers_by_json_tag, get_papers_by_source_fks, list_papers,
    list_papers_sorted, list_pdf_papers, DoiVersionCandidate, PaperSort, PAPER_COLUMNS_NO_TEXT,
};
pub use roots::{ensure_paper_root, get_paper_root, get_source_id, sfks_to_source_ids, PaperRoot};
pub use trash::{
    hard_delete_paper, is_paper_deleted, list_deleted_papers, restore_paper, soft_delete_paper,
    DeletedPaper,
};
pub(crate) use write::write_paper_version_in_tx;
pub use write::{add_paper_tags, remove_paper_tags, repair_paper, save_paper_metadata};

/// Shared fixtures for the submodules' test mods.
#[cfg(test)]
mod testutil {
    use crate::models::PaperMetadata;
    use chrono::NaiveDate;
    use rusqlite::{params, Connection};

    pub(super) fn seed(conn: &Connection) {
        conn.execute(
            "INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES ('arxiv:2204.12985')",
            [],
        )
        .unwrap();
        let fk = conn.last_insert_rowid();
        for (ver, title, pub_date) in [(1, "V1", "2024-01-01"), (2, "V2", "2024-03-05")] {
            conn.execute(
                "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, CATEGORY, HAS_PDF, SOURCE_FK) \
                 VALUES ('arxiv:2204.12985', ?1, ?2, 'cs.LG', 1, ?3)",
                params![ver, title, fk],
            )
            .unwrap();
            let pid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO PAPER_META (PAPER_ID, URL, PUBLISHED, CATEGORIES, SUMMARY, AUTHORS, TAGS, DOI) \
                 VALUES (?1, 'http://x', ?2, '[\"cs.LG\",\"cs.AI\"]', 'sum', '[\"Alice\",\"Bob\"]', '[\"ml\"]', '10.1/x')",
                params![pid, pub_date],
            )
            .unwrap();
        }
    }

    pub(super) fn meta(source_id: &str, version: i64) -> PaperMetadata {
        PaperMetadata {
            source_id: source_id.into(),
            version,
            title: "T".into(),
            authors: vec!["Alice".into(), "Bob".into()],
            published: NaiveDate::from_ymd_opt(2024, 3, 5).unwrap(),
            updated: None,
            summary: "sum".into(),
            category: Some("cs.LG".into()),
            categories: Some(vec!["cs.LG".into(), "cs.AI".into()]),
            doi: Some("10.1/x".into()),
            journal_ref: None,
            comment: None,
            url: Some("http://x".into()),
            tags: Some(vec!["ml".into()]),
            source: Some("arxiv".into()),
            author_orcids: None,
        }
    }

    pub(super) fn count(conn: &Connection, sql: &str, sid: &str) -> i64 {
        conn.query_row(sql, [sid], |r| r.get(0)).unwrap()
    }
}
