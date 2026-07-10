//! VERSION_CHECK queries — per-root arXiv version-poll bookkeeping (one row per
//! PAPER_ROOTS root: LAST_CHECKED_AT + un-acknowledged NEW_VERSION flag). The
//! poll pass itself lives in `service::version_monitor`; this module owns the SQL.

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::error::Result;

pub const MAX_VERSION_CHECK_BATCH: i64 = 100;

/// One saved arXiv paper due for a check: its root + the newest stored version.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub source_fk: i64,
    pub source_id: String,
    pub known_version: i64,
}

/// A newly discovered (and now captured) version, for the poll report / badge list.
#[derive(Debug, Clone, Serialize)]
pub struct NewVersion {
    pub source_fk: i64,
    pub source_id: String,
    pub title: String,
    pub version: i64,
}

/// The stalest `limit` active arXiv roots: never-checked first, then oldest
/// LAST_CHECKED_AT. Roots with no stored version (nothing to compare) and
/// non-arXiv/deleted roots are excluded.
pub fn stale_candidates(conn: &Connection, limit: i64) -> Result<Vec<Candidate>> {
    let limit = limit.clamp(1, MAX_VERSION_CHECK_BATCH);
    let mut stmt = conn.prepare(
        "SELECT r.SOURCE_FK, r.SOURCE_ID, MAX(p.VERSION)
         FROM PAPER_ROOTS r
         JOIN PAPER p ON p.SOURCE_FK = r.SOURCE_FK
         LEFT JOIN VERSION_CHECK v ON v.SOURCE_FK = r.SOURCE_FK
         WHERE r.STATUS = 'active' AND r.SOURCE_ID LIKE 'arxiv:%'
         GROUP BY r.SOURCE_FK
         ORDER BY (v.LAST_CHECKED_AT IS NOT NULL), v.LAST_CHECKED_AT ASC, r.SOURCE_FK ASC
         LIMIT ?1",
    )?;
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
