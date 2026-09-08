//! db-admin service — connection bootstrap and the backup/restore front door.
//!
//! ADR 0010: consumers must not open connections or call `storage::` themselves.
//!
//! * `open_app_db` — open `config::db_path()` and bring the schema forward.
//! * `backup` / `validate_backup_source` — pass-throughs so nobody reaches past.
//! * `restore_in_place` — park the caller's live handle, swap the file, reopen;
//!   core's `restore` refuses while any handle is open, so the parking dance is mandatory.

use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::config;
use crate::error::{CoreError, Result};
use crate::storage::{self, backup::BackupInfo};

/// Open the app database at its configured path with the schema applied.
pub fn open_app_db() -> Result<Connection> {
    let conn = storage::open(&config::db_path())?;
    storage::init_db(&conn)?;
    Ok(conn)
}

/// `path` with its parent canonicalized, filename kept — so a destination that does
/// not exist yet still compares against the live DB.
fn canon_or_raw(path: &Path) -> PathBuf {
    path.parent()
        .and_then(|p| p.canonicalize().ok())
        .zip(path.file_name())
        .map(|(canon_parent, fname)| canon_parent.join(fname))
        .unwrap_or_else(|| path.to_path_buf())
}

/// Refuse a backup/restore path that is relative, non-UTF-8, or resolves to the live
/// database file itself. `field`/`role` name the offending input in the message.
/// `Validation` → 422, `BadRequest` → 400 (the route contract this came from).
pub fn reject_live_db(path: &Path, field: &str, role: &str) -> Result<()> {
    if !path.is_absolute() {
        return Err(CoreError::Validation(format!("{field} must be absolute")));
    }
    if path.to_str().is_none() {
        return Err(CoreError::Validation(format!("{field} is not valid UTF-8")));
    }
    let (a, b) = (canon_or_raw(path), canon_or_raw(&config::db_path()));
    // Case-insensitive comparison only on case-insensitive filesystems.
    let same = if cfg!(windows) || cfg!(target_os = "macos") {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    };
    if same {
        return Err(CoreError::BadRequest(format!(
            "{role} is the live database itself — choose another file"
        )));
    }
    Ok(())
}

/// Snapshot the live DB to `dest` (must not exist). See `storage::backup`.
pub fn backup(conn: &Connection, dest: &Path) -> Result<BackupInfo> {
    storage::backup(conn, dest)
}

/// Reject a restore source that is not a usable SQLite snapshot. Callers run this
/// before parking their live handle so a bad path fails cheaply.
pub fn validate_backup_source(src: &Path) -> Result<()> {
    storage::validate_backup_source(src)
}

/// Replace the live database with the `src` snapshot, leaving `conn` pointing at a
/// working handle either way.
///
/// `conn` is parked on an in-memory DB and closed first, so core's
/// `ensure_no_live_connections` only has to refuse *other* processes. The reopen
/// runs whether or not the swap succeeded — a caller left holding a dead handle is
/// worse than a refused restore — and re-runs `init_db`, migrating an older
/// snapshot forward. The restore's own error is returned after the reopen.
///
/// Blocking: two full-file copies plus a rename. Async callers should run this on a
/// blocking thread rather than holding a runtime worker.
pub fn restore_in_place(conn: &mut Connection, src: &Path) -> Result<()> {
    let db_path = config::db_path();
    let parked = storage::open_in_memory()
        .map_err(|e| CoreError::Internal(format!("could not park live connection: {e}")))?;
    let live = std::mem::replace(conn, parked);
    if let Err((returned, e)) = live.close() {
        *conn = returned;
        return Err(CoreError::Internal(format!(
            "could not close the live database: {e}"
        )));
    }
    let result = storage::restore(src, &db_path);
    *conn = storage::open(&db_path)
        .and_then(|fresh| storage::init_db(&fresh).map(|()| fresh))
        .map_err(|e| {
            CoreError::Internal(format!(
                "could not reopen the database — restart linXiv: {e}"
            ))
        })?;
    result
}

/// Restore onto a database nobody currently holds open — the CLI's `restore`,
/// which deliberately runs before the DB is opened so a corrupted library can
/// still be recovered.
pub fn restore_closed(src: &Path) -> Result<()> {
    storage::restore(src, &config::db_path())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LINXIV_DATA_DIR is process-global, so the whole round trip is one test.
    #[test]
    fn restore_in_place_swaps_the_file_and_always_leaves_a_live_handle() {
        let dir = std::env::temp_dir().join(format!("linxiv-db-admin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("LINXIV_DATA_DIR", &dir);

        let mut conn = open_app_db().unwrap();
        conn.execute_batch("CREATE TABLE t (v TEXT); INSERT INTO t (v) VALUES ('before');")
            .unwrap();
        let snapshot = dir.join("snap.db");
        backup(&conn, &snapshot).unwrap();

        // Diverge the live DB, then restore the snapshot over it.
        conn.execute("UPDATE t SET v = 'after'", []).unwrap();
        restore_in_place(&mut conn, &snapshot).unwrap();
        let v: String = conn.query_row("SELECT v FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(v, "before");

        // A refused restore still hands back a usable connection.
        let missing = dir.join("nope.db");
        assert!(restore_in_place(&mut conn, &missing).is_err());
        assert!(conn
            .query_row("SELECT v FROM t", [], |r| r.get::<_, String>(0))
            .is_ok());

        std::env::remove_var("LINXIV_DATA_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
