//! Connection + column-type converters. Plan §5.3 + D4/D5. LIST/DATE/TIMESTAMP/
//! BOOL conversion is explicit at the row-mapping site via the helper fns below;
//! the named queries call them when shaping models.

use std::path::Path;

use chrono::{DateTime, NaiveDate, NaiveDateTime};
use rusqlite::{Connection, Transaction};

use crate::error::{CoreError, Result};

/// Open a connection with `PRAGMA foreign_keys = ON`.
/// NON-NEGOTIABLE: the PRAGMA is per-connection and defaults OFF — without it
/// every `ON DELETE CASCADE` silently no-ops. It must run on EVERY connection.
/// `busy_timeout` waits for a writer in another process (app/CLI/MCP share the
/// file). WAL is best-effort (`let _`): entering it can be SQLITE_BUSY, and
/// `backup.rs::ensure_no_live_connections` relies on `open` failing ONLY for
/// unopenable files; the mode is sticky per DB file, synchronous stays FULL.
pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")?;
    let _ = conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()));
    Ok(conn)
}

/// In-memory DB (tests / ephemeral) — same FK PRAGMA contract as `open`.
pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    Ok(conn)
}

/// Run `f` inside a transaction, committing on `Ok`, rolling back on `Err`/drop.
/// IMMEDIATE, not DEFERRED: SQLite does not invoke the busy handler for a
/// read→write promotion (instant SQLITE_BUSY); taking the write lock up front
/// lets `busy_timeout` cover a writer in another process (app/CLI/MCP).
pub fn transaction<T>(
    conn: &mut Connection,
    f: impl FnOnce(&Transaction) -> Result<T>,
) -> Result<T> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let out = f(&tx)?;
    tx.commit()?;
    Ok(out)
}

// ── decltype converters (explicit; no register_converter in rusqlite) ─────────

/// LIST column ⇄ Vec<String>, stored as a JSON array TEXT.
/// to_sql never fails for a string vec; defaults to `[]` rather than panicking.
pub fn list_to_sql(v: &[String]) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string())
}

pub fn list_from_sql(s: &str) -> Result<Vec<String>> {
    serde_json::from_str(s).map_err(|e| CoreError::Internal(e.to_string()))
}

/// DATE column ⇄ NaiveDate as ISO-8601 `YYYY-MM-DD`.
pub fn date_to_sql(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

pub fn date_from_sql(s: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
        .map_err(|e| CoreError::Internal(format!("bad DATE {s:?}: {e}")))
}

/// TIMESTAMP ⇄ NaiveDateTime. Write keeps the sub-second fraction (`%.f`, omitted
/// at zero) so microsecond precision is not dropped; read accepts BOTH the 'T'
/// form and the space form `datetime('now')` emits.
pub fn timestamp_to_sql(dt: NaiveDateTime) -> String {
    dt.format("%Y-%m-%dT%H:%M:%S%.f").to_string()
}

pub fn timestamp_from_sql(s: &str) -> Result<NaiveDateTime> {
    let t = s.trim();
    let norm = t.replacen('T', " ", 1);
    NaiveDateTime::parse_from_str(&norm, "%Y-%m-%d %H:%M:%S%.f")
        // Legacy rows carry microseconds + a UTC offset (e.g.
        // "2026-06-04T03:10:47.041006+00:00"); RFC3339-parse those and
        // normalize to naive UTC so the offset isn't trailing input.
        .or_else(|_| DateTime::parse_from_rfc3339(t).map(|dt| dt.naive_utc()))
        .map_err(|e| CoreError::Internal(format!("bad TIMESTAMP {s:?}: {e}")))
}

pub fn bool_from_sql(i: i64) -> bool {
    i != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fk_pragma_on_and_converters_roundtrip() {
        let conn = open_in_memory().unwrap();
        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1, "foreign_keys must be ON on every connection");

        assert_eq!(
            list_from_sql(&list_to_sql(&["a".into(), "b".into()])).unwrap(),
            vec!["a".to_string(), "b".to_string()]
        );
        let d = NaiveDate::from_ymd_opt(2024, 3, 5).unwrap();
        assert_eq!(date_from_sql(&date_to_sql(d)).unwrap(), d);
        // read tolerates both separators
        let dt = NaiveDateTime::parse_from_str("2024-03-05 12:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
        assert_eq!(timestamp_from_sql("2024-03-05T12:00:00").unwrap(), dt);
        assert_eq!(timestamp_from_sql("2024-03-05 12:00:00").unwrap(), dt);
        assert!(bool_from_sql(1));
        assert!(!bool_from_sql(0));
    }

    #[test]
    fn file_backed_open_runs_in_wal_mode() {
        let dir = std::env::temp_dir().join(format!("linxiv-db-wal-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mode: String = open(&dir.join("papers.db"))
            .unwrap()
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn timestamp_from_sql_accepts_python_isoformat_offset() {
        // Legacy row: microseconds + a UTC offset that used to be "trailing input".
        let utc = NaiveDate::from_ymd_opt(2026, 6, 4)
            .unwrap()
            .and_hms_micro_opt(3, 10, 47, 41006)
            .unwrap();
        assert_eq!(
            timestamp_from_sql("2026-06-04T03:10:47.041006+00:00").unwrap(),
            utc
        );
        // A non-zero offset is normalized to naive UTC (03:10 -02:00 -> 05:10Z).
        assert_eq!(
            timestamp_from_sql("2026-06-04T03:10:47-02:00").unwrap(),
            NaiveDate::from_ymd_opt(2026, 6, 4)
                .unwrap()
                .and_hms_opt(5, 10, 47)
                .unwrap()
        );
    }
}
