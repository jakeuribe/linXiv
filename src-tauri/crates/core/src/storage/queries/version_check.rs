//! VERSION_CHECK queries — per-root arXiv version-poll bookkeeping (one row per
//! PAPER_ROOTS root: LAST_CHECKED_AT + un-acknowledged NEW_VERSION flag). The
//! poll pass itself lives in `service::version_monitor`; this module owns the SQL.

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::error::Result;
use crate::models::ARXIV_ID_PREFIX;

pub const MAX_VERSION_CHECK_BATCH: i64 = 100;

/// One saved arXiv paper due for a check: its root + the newest stored version.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub source_fk: i64,
    pub source_id: String,
    pub known_version: i64,
}

/// A newly discovered (and now captured) version, for the poll report / badge list.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
pub struct NewVersion {
    pub source_fk: i64,
    pub source_id: String,
    pub title: String,
    pub version: i64,
}

/// The stalest `limit` active arXiv roots: never-checked first, then oldest
/// LAST_CHECKED_AT. Roots with no stored version (nothing to compare) and
/// non-arXiv/deleted roots are excluded.
///
/// The arXiv pattern is built from `models::ARXIV_ID_PREFIX`, the same constant
/// `service::paper::source_fetch_url` tests. GLOB, not LIKE: LIKE is
/// ASCII-case-insensitive in SQLite.
pub fn stale_candidates(conn: &Connection, limit: i64) -> Result<Vec<Candidate>> {
    let limit = limit.clamp(1, MAX_VERSION_CHECK_BATCH);
    let mut stmt = conn.prepare(&format!(
        "SELECT r.SOURCE_FK, r.SOURCE_ID, MAX(p.VERSION)
         FROM PAPER_ROOTS r
         JOIN PAPER p ON p.SOURCE_FK = r.SOURCE_FK
         LEFT JOIN VERSION_CHECK v ON v.SOURCE_FK = r.SOURCE_FK
         WHERE r.STATUS = 'active' AND r.SOURCE_ID GLOB '{ARXIV_ID_PREFIX}*'
         GROUP BY r.SOURCE_FK
         ORDER BY (v.LAST_CHECKED_AT IS NOT NULL), v.LAST_CHECKED_AT ASC, r.SOURCE_FK ASC
         LIMIT ?1"
    ))?;
    let rows = stmt.query_map([limit], |r| {
        Ok(Candidate {
            source_fk: r.get(0)?,
            source_id: r.get(1)?,
            known_version: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Record one check: bump LAST_CHECKED_AT; set NEW_VERSION when a discovery was
/// made, otherwise keep any still-unacknowledged flag from an earlier pass.
pub fn record_check(conn: &Connection, source_fk: i64, new_version: Option<i64>) -> Result<()> {
    conn.execute(
        "INSERT INTO VERSION_CHECK (SOURCE_FK, LAST_CHECKED_AT, NEW_VERSION)
         VALUES (?1, datetime('now'), ?2)
         ON CONFLICT(SOURCE_FK) DO UPDATE SET
           LAST_CHECKED_AT = excluded.LAST_CHECKED_AT,
           NEW_VERSION = COALESCE(excluded.NEW_VERSION, NEW_VERSION)",
        params![source_fk, new_version],
    )?;
    Ok(())
}

/// Papers with an un-acknowledged newly-found version (the badge/list surface).
pub fn list_new_versions(conn: &Connection) -> Result<Vec<NewVersion>> {
    let mut stmt = conn.prepare(
        "SELECT v.SOURCE_FK, r.SOURCE_ID, p.TITLE, v.NEW_VERSION
         FROM VERSION_CHECK v
         JOIN PAPER_ROOTS r ON r.SOURCE_FK = v.SOURCE_FK
         JOIN PAPER p ON p.SOURCE_FK = v.SOURCE_FK AND p.VERSION = v.NEW_VERSION
         WHERE v.NEW_VERSION IS NOT NULL AND r.STATUS = 'active'
         ORDER BY v.LAST_CHECKED_AT DESC
         LIMIT 100",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(NewVersion {
            source_fk: r.get(0)?,
            source_id: r.get(1)?,
            title: r.get(2)?,
            version: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Clear the new-version flag for one root (user dismissed / viewed it).
/// Returns false when there was nothing to clear.
pub fn ack(conn: &Connection, source_fk: i64) -> Result<bool> {
    let n = conn.execute(
        "UPDATE VERSION_CHECK SET NEW_VERSION = NULL
         WHERE SOURCE_FK = ?1 AND NEW_VERSION IS NOT NULL",
        [source_fk],
    )?;
    Ok(n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{self, db};

    /// Insert an active root + one PAPER row per version. Returns the SOURCE_FK.
    fn seed_root(conn: &Connection, source_id: &str, versions: &[i64]) -> i64 {
        conn.execute(
            "INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES (?1)",
            [source_id],
        )
        .unwrap();
        let fk = conn.last_insert_rowid();
        for v in versions {
            conn.execute(
                "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, SOURCE_FK)
                 VALUES (?1, ?2, ?3, ?4)",
                params![source_id, v, format!("{source_id} v{v}"), fk],
            )
            .unwrap();
        }
        fk
    }

    fn candidate_ids(conn: &Connection) -> Vec<String> {
        stale_candidates(conn, MAX_VERSION_CHECK_BATCH)
            .unwrap()
            .into_iter()
            .map(|c| c.source_id)
            .collect()
    }

    /// The `GLOB 'arxiv:*'` filter is an ANCHORED, CASE-SENSITIVE prefix match.
    ///
    /// This is the test that fails if anyone "simplifies" GLOB back to
    /// `LIKE 'arxiv:%'`: SQLite's LIKE is ASCII-case-insensitive, so the
    /// `ARXIV:` / `arXiv:` roots below would start being polled as arXiv papers.
    /// The `doi:` row carries `arxiv:` mid-string to pin that the match is
    /// anchored (a bare `%arxiv:%` / `*arxiv:*` would sweep it in), and the
    /// `arxiv-sanity:` / `arxivlike:` rows pin that the colon is part of the
    /// prefix rather than the start of a wildcard run.
    #[test]
    fn stale_candidates_matches_arxiv_prefix_anchored_and_case_sensitively() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();

        seed_root(&conn, "arxiv:2401.00001", &[1]);
        for reject in [
            "ARXIV:2401.00002",             // LIKE would match this; GLOB must not
            "arXiv:2401.00003",             // the canonical arXiv branding spelling
            "doi:10.1000/arxiv:2401.00004", // contains the prefix, doesn't start with it
            "arxiv-sanity:2401.00005",      // near-miss prefix, no colon after `arxiv`
            "arxivlike:2401.00006",         // near-miss prefix, no separator at all
            "openalex:W123",
            "local:deadbeef",
            ":arxiv:2401.00007", // prefix present but not at position 0
        ] {
            seed_root(&conn, reject, &[1]);
        }

        assert_eq!(candidate_ids(&conn), vec!["arxiv:2401.00001".to_string()]);
    }

    /// GLOB's own metacharacters (`*`, `?`, `[…]`) live in the PATTERN, never in
    /// the data — a stored id containing them is matched literally, and a stored
    /// id is never treated as a pattern itself.
    #[test]
    fn stale_candidates_treats_wildcard_chars_in_data_literally() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();

        seed_root(&conn, "arxiv:*", &[1]); // in-prefix: matched, but only literally
        seed_root(&conn, "arxi?:2401.00001", &[1]); // `?` must not stand in for `v`
        seed_root(&conn, "[a]rxiv:2401.00002", &[1]); // `[…]` must not act as a class

        assert_eq!(candidate_ids(&conn), vec!["arxiv:*".to_string()]);
    }

    /// Deleted roots and roots with no stored PAPER row (nothing to compare a
    /// poll result against) are both excluded; `known_version` is the newest
    /// stored version, not the first or the count.
    #[test]
    fn stale_candidates_excludes_deleted_and_versionless_roots() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();

        seed_root(&conn, "arxiv:kept", &[1, 3, 2]);
        seed_root(&conn, "arxiv:no-versions", &[]);
        let deleted = seed_root(&conn, "arxiv:deleted", &[1]);
        conn.execute(
            "UPDATE PAPER_ROOTS SET STATUS = 'deleted' WHERE SOURCE_FK = ?1",
            [deleted],
        )
        .unwrap();

        let got = stale_candidates(&conn, MAX_VERSION_CHECK_BATCH).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].source_id, "arxiv:kept");
        assert_eq!(
            got[0].known_version, 3,
            "known_version must be MAX(VERSION)"
        );
    }

    /// Never-checked roots come first, then the stalest LAST_CHECKED_AT; ties
    /// (including the never-checked bucket) break on SOURCE_FK so the rotation
    /// is deterministic rather than whatever order the join happens to emit.
    #[test]
    fn stale_candidates_orders_never_checked_first_then_stalest() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();

        let recent = seed_root(&conn, "arxiv:recent", &[1]);
        let old = seed_root(&conn, "arxiv:old", &[1]);
        let fresh_a = seed_root(&conn, "arxiv:fresh-a", &[1]);
        let fresh_b = seed_root(&conn, "arxiv:fresh-b", &[1]);

        // datetime('now') has second resolution, so pin the timestamps explicitly
        // rather than relying on two record_check calls landing apart.
        for (fk, at) in [
            (recent, "2026-01-02 00:00:00"),
            (old, "2026-01-01 00:00:00"),
        ] {
            record_check(&conn, fk, None).unwrap();
            conn.execute(
                "UPDATE VERSION_CHECK SET LAST_CHECKED_AT = ?2 WHERE SOURCE_FK = ?1",
                params![fk, at],
            )
            .unwrap();
        }

        let got: Vec<i64> = stale_candidates(&conn, MAX_VERSION_CHECK_BATCH)
            .unwrap()
            .into_iter()
            .map(|c| c.source_fk)
            .collect();
        assert_eq!(got, vec![fresh_a, fresh_b, old, recent]);
    }

    /// `limit` is clamped into `1..=MAX_VERSION_CHECK_BATCH` — a caller passing 0
    /// or a negative must still make progress, and one passing a huge number must
    /// not drag the whole library into a single poll pass.
    #[test]
    fn stale_candidates_clamps_limit() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        for i in 0..3 {
            seed_root(&conn, &format!("arxiv:{i}"), &[1]);
        }

        assert_eq!(stale_candidates(&conn, 0).unwrap().len(), 1);
        assert_eq!(stale_candidates(&conn, -7).unwrap().len(), 1);
        assert_eq!(stale_candidates(&conn, 2).unwrap().len(), 2);
        assert_eq!(stale_candidates(&conn, i64::MAX).unwrap().len(), 3);
    }

    /// A repeat check bumps LAST_CHECKED_AT but must not silently drop a flag the
    /// user hasn't acknowledged yet (the COALESCE in the upsert) — that's the
    /// difference between "you have a new version" and a badge that vanishes on
    /// the next poll pass.
    #[test]
    fn record_check_upserts_and_preserves_unacked_flag() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        let fk = seed_root(&conn, "arxiv:2401.00001", &[1, 2]);

        record_check(&conn, fk, None).unwrap();
        let flag: Option<i64> = conn
            .query_row("SELECT NEW_VERSION FROM VERSION_CHECK", [], |r| r.get(0))
            .unwrap();
        assert_eq!(flag, None);

        record_check(&conn, fk, Some(2)).unwrap();
        conn.execute(
            "UPDATE VERSION_CHECK SET LAST_CHECKED_AT = '2026-01-01 00:00:00'",
            [],
        )
        .unwrap();

        // A later no-discovery pass keeps the un-acked 2 and still bumps the clock.
        record_check(&conn, fk, None).unwrap();
        let (flag, checked): (Option<i64>, String) = conn
            .query_row(
                "SELECT NEW_VERSION, LAST_CHECKED_AT FROM VERSION_CHECK",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(flag, Some(2), "an un-acked flag must survive a quiet pass");
        assert_ne!(checked, "2026-01-01 00:00:00");

        // Exactly one row per root: the ON CONFLICT is an update, not an insert.
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM VERSION_CHECK", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1);
    }

    /// The badge list shows only un-acked discoveries on active roots whose new
    /// version was actually captured as a PAPER row, newest check first.
    #[test]
    fn list_new_versions_filters_and_orders() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();

        let a = seed_root(&conn, "arxiv:a", &[1, 2]);
        let b = seed_root(&conn, "arxiv:b", &[1, 5]);
        let acked = seed_root(&conn, "arxiv:acked", &[1, 2]);
        let deleted = seed_root(&conn, "arxiv:deleted", &[1, 2]);
        let uncaptured = seed_root(&conn, "arxiv:uncaptured", &[1]);

        for (fk, v, at) in [
            (a, 2, "2026-01-01 00:00:00"),
            (b, 5, "2026-01-02 00:00:00"),
            (acked, 2, "2026-01-03 00:00:00"),
            (deleted, 2, "2026-01-04 00:00:00"),
            (uncaptured, 2, "2026-01-05 00:00:00"), // flagged, no PAPER row at v2
        ] {
            record_check(&conn, fk, Some(v)).unwrap();
            conn.execute(
                "UPDATE VERSION_CHECK SET LAST_CHECKED_AT = ?2 WHERE SOURCE_FK = ?1",
                params![fk, at],
            )
            .unwrap();
        }
        assert!(ack(&conn, acked).unwrap());
        conn.execute(
            "UPDATE PAPER_ROOTS SET STATUS = 'deleted' WHERE SOURCE_FK = ?1",
            [deleted],
        )
        .unwrap();

        let got = list_new_versions(&conn).unwrap();
        assert_eq!(
            got.iter().map(|n| n.source_id.as_str()).collect::<Vec<_>>(),
            vec!["arxiv:b", "arxiv:a"],
            "newest check first; acked / deleted / uncaptured excluded"
        );
        assert_eq!(got[0].version, 5);
        assert_eq!(got[0].source_fk, b);
        assert_eq!(
            got[0].title, "arxiv:b v5",
            "title comes from the new version"
        );
    }

    #[test]
    fn ack_clears_once_and_reports_whether_it_did() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        let fk = seed_root(&conn, "arxiv:2401.00001", &[1, 2]);

        assert!(!ack(&conn, fk).unwrap(), "no VERSION_CHECK row yet");
        record_check(&conn, fk, None).unwrap();
        assert!(!ack(&conn, fk).unwrap(), "row exists but nothing flagged");

        record_check(&conn, fk, Some(2)).unwrap();
        assert!(ack(&conn, fk).unwrap());
        assert!(!ack(&conn, fk).unwrap(), "second ack is a no-op");
        assert!(list_new_versions(&conn).unwrap().is_empty());
        assert!(!ack(&conn, 9999).unwrap(), "unknown root");
    }

    /// VERSION_CHECK rows are per-root bookkeeping, not user data: hard-deleting
    /// the root must take the poll state with it (ON DELETE CASCADE), or the next
    /// root to reuse that SOURCE_FK inherits a stale flag.
    #[test]
    fn version_check_row_cascades_with_its_root() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        let fk = seed_root(&conn, "arxiv:2401.00001", &[1, 2]);
        record_check(&conn, fk, Some(2)).unwrap();

        conn.execute("DELETE FROM PAPER_ROOTS WHERE SOURCE_FK = ?1", [fk])
            .unwrap();

        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM VERSION_CHECK", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0);
    }
}
