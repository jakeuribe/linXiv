//! version_monitor — RSS-style polling for new arXiv versions, separate from the
//! opportunistic capture that happens on refetch. Each pass checks the stalest N
//! saved arXiv papers (tracked per-root in VERSION_CHECK) and records any version
//! newer than the max already stored, capturing it through the EXISTING path
//! (`save_paper_metadata`, the same INSERT-OR-IGNORE-per-version write refetch uses).
//!
//! The pass itself is orchestrated by the caller (route): `stale_candidates`
//! → one batched `sources::arxiv::fetch_by_ids` → `apply_results`. Keeping the
//! network hop outside means everything here is sync + unit-testable offline.

use rusqlite::Connection;

use crate::error::Result;
use crate::models::PaperMetadata;
use crate::storage::queries::paper as store;
pub use crate::storage::queries::version_check::{
    ack, list_new_versions, record_check, stale_candidates, Candidate, NewVersion,
    MAX_VERSION_CHECK_BATCH,
};

/// True iff the fetched version should be captured.
fn is_newer(fetched: i64, known: i64) -> bool {
    fetched > known
}

/// Process one candidate: check root status. For roots that are active and
/// resolvable, save any newer version and record the check (with the new version
/// if found, or None if not). Errors are caught and logged by apply_results,
/// allowing the candidate to rotate out instead of blocking future checks.
/// Skips both save and record for deleted or inactive roots.
fn process_candidate(
    conn: &mut Connection,
    cand: &Candidate,
    fetched: &[PaperMetadata],
) -> Result<Option<NewVersion>> {
    // Check root status unconditionally; skip both save and record if deleted or inactive.
    let Some(root) = store::get_paper_root(conn, &cand.source_id)? else {
        return Ok(None);
    };
    if root.status != "active" {
        return Ok(None);
    }
    let newer = fetched
        .iter()
        .find(|m| m.source_id == cand.source_id)
        .filter(|m| is_newer(m.version, cand.known_version));
    if let Some(m) = newer {
        // ponytail: flag-then-save so a crash re-checks instead of silently raising
        // known_version; one real tx needs a tx-level write_paper_version in paper.rs.
        record_check(conn, root.source_fk, Some(m.version))?;
        store::save_paper_metadata(conn, m, None)?;
        let result = NewVersion {
            source_fk: root.source_fk,
            source_id: cand.source_id.clone(),
            title: m.title.clone(),
            version: m.version,
        };
        return Ok(Some(result));
    }
    record_check(conn, root.source_fk, None)?;
    Ok(None)
}

/// Apply one pass's fetched metadata: for every candidate with an active root,
/// capture a newer-than-known version and record the check (with or without the
/// new version). Candidates with missing/inactive roots are skipped entirely.
/// Per-candidate errors are logged and swallowed so the pass continues.
pub fn apply_results(
    conn: &mut Connection,
    candidates: &[Candidate],
    fetched: &[PaperMetadata],
) -> Result<Vec<NewVersion>> {
    let mut found = Vec::new();
    for cand in candidates {
        match process_candidate(conn, cand, fetched) {
            Ok(Some(new_version)) => found.push(new_version),
            Ok(None) => {}
            Err(e) => {
                tracing::warn!("error checking candidate {}: {}", cand.source_id, e);
            }
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{db::open_in_memory, init_db};
    use chrono::NaiveDate;

    fn mem() -> Connection {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn
    }

    fn meta(source_id: &str, version: i64) -> PaperMetadata {
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
        }
    }

    fn save(conn: &mut Connection, source_id: &str, version: i64) {
        store::save_paper_metadata(conn, &meta(source_id, version), None).unwrap();
    }

    fn fk(conn: &Connection, source_id: &str) -> i64 {
        store::get_paper_root(conn, source_id)
            .unwrap()
            .unwrap()
            .source_fk
    }

    #[test]
    fn stale_candidates_selects_arxiv_never_checked_then_oldest() {
        let mut conn = mem();
        save(&mut conn, "arxiv:a", 1);
        save(&mut conn, "arxiv:b", 2);
        save(&mut conn, "arxiv:c", 1);
        save(&mut conn, "local:x", 1); // non-arXiv: excluded
        save(&mut conn, "arxiv:gone", 1);
        store::soft_delete_paper(&mut conn, "arxiv:gone").unwrap(); // deleted: excluded

        // b checked long ago, c checked just now, a never checked.
        record_check(&conn, fk(&conn, "arxiv:b"), None).unwrap();
        conn.execute(
            "UPDATE VERSION_CHECK SET LAST_CHECKED_AT = '2000-01-01 00:00:00' WHERE SOURCE_FK = ?1",
            [fk(&conn, "arxiv:b")],
        )
        .unwrap();
        record_check(&conn, fk(&conn, "arxiv:c"), None).unwrap();

        let ids: Vec<String> = stale_candidates(&conn, 10)
            .unwrap()
            .into_iter()
            .map(|c| c.source_id)
            .collect();
        assert_eq!(ids, vec!["arxiv:a", "arxiv:b", "arxiv:c"]);

        // known_version is the max stored version; limit is respected.
        let top = stale_candidates(&conn, 1).unwrap();
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].source_id, "arxiv:a");
        assert_eq!(top[0].known_version, 1);
    }

    #[test]
    fn apply_results_captures_only_newer_and_rotates_all() {
        let mut conn = mem();
        save(&mut conn, "arxiv:up", 1);
        save(&mut conn, "arxiv:same", 3);
        save(&mut conn, "arxiv:silent", 1); // arXiv returns nothing for it

        let cands = stale_candidates(&conn, 10).unwrap();
        let fetched = vec![meta("arxiv:up", 2), meta("arxiv:same", 3)];
        let found = apply_results(&mut conn, &cands, &fetched).unwrap();

        // Only the strictly-newer version was captured + flagged.
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].source_id, "arxiv:up");
        assert_eq!(found[0].version, 2);
        assert_eq!(store::get_all_versions(&conn, "arxiv:up").unwrap().len(), 2);
        assert_eq!(
            store::get_all_versions(&conn, "arxiv:same").unwrap().len(),
            1
        );

        // Every candidate (even the unanswered one) got LAST_CHECKED_AT, so a
        // fresh never-checked paper now outranks all three.
        save(&mut conn, "arxiv:fresh", 1);
        let next = stale_candidates(&conn, 1).unwrap();
        assert_eq!(next[0].source_id, "arxiv:fresh");

        // The discovery is listed until acked; a later no-news pass keeps it.
        let listed = list_new_versions(&conn).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].source_id, "arxiv:up");
        assert_eq!(listed[0].version, 2);
        record_check(&conn, fk(&conn, "arxiv:up"), None).unwrap();
        assert_eq!(list_new_versions(&conn).unwrap().len(), 1);

        assert!(ack(&conn, fk(&conn, "arxiv:up")).unwrap());
        assert!(list_new_versions(&conn).unwrap().is_empty());
        assert!(!ack(&conn, fk(&conn, "arxiv:up")).unwrap());
    }

    #[test]
    fn apply_results_skips_deleted_candidates_no_fk_crash() {
        let mut conn = mem();
        save(&mut conn, "arxiv:live", 1);
        save(&mut conn, "arxiv:deleted", 1);

        let cands = stale_candidates(&conn, 10).unwrap();
        assert_eq!(cands.len(), 2);

        // Delete one root's row (CASCADE deletes VERSION_CHECK and PAPER dependents).
        conn.execute(
            "DELETE FROM PAPER_ROOTS WHERE SOURCE_ID = ?1",
            ["arxiv:deleted"],
        )
        .unwrap();

        // apply_results with metadata for both: should skip the deleted one, not crash.
        let fetched = vec![meta("arxiv:live", 2), meta("arxiv:deleted", 2)];
        let found = apply_results(&mut conn, &cands, &fetched).unwrap();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].source_id, "arxiv:live");
        assert_eq!(found[0].version, 2);
    }
}
