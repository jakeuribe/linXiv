//! DB backup / restore — a library survives corruption or a bad migration.
//!
//! `backup` uses SQLite `VACUUM INTO`: it writes a consistent, self-contained
//! snapshot of the live DB (all committed pages, no WAL leftovers) in a single
//! atomic statement.
//! `restore` validates the snapshot is a real SQLite DB, then replaces the file.

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// Result of a successful `backup`: where it landed and how big it is.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupInfo {
    pub path: PathBuf,
    pub bytes: u64,
}

/// Snapshot the live DB to `dest` via `VACUUM INTO` (atomic + consistent).
/// `dest` must not already exist — SQLite refuses to overwrite.
pub fn backup(conn: &Connection, dest: &Path) -> Result<BackupInfo> {
    if dest.exists() {
        return Err(CoreError::BadRequest(format!(
            "destination already exists: {} — remove it or choose another path",
            dest.display()
        )));
    }
    if let Err(e) = conn.execute("VACUUM INTO ?1", [&dest.to_string_lossy()]) {
        let _ = std::fs::remove_file(dest);
        return Err(e.into());
    }
    let bytes = match std::fs::metadata(dest) {
        Ok(m) => m.len(),
        Err(e) => {
            let _ = std::fs::remove_file(dest);
            return Err(CoreError::Internal(format!("backup written but unreadable: {e}")));
        }
    };
    Ok(BackupInfo {
        path: dest.to_path_buf(),
        bytes,
    })
}

/// True when the error is SQLite's "another handle holds a lock".
fn is_busy(e: &rusqlite::Error) -> bool {
    matches!(
        e.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
    )
}

/// Refuse to swap the DB file out from under a live SQLite handle: an open
/// handle keeps writing to the replaced inode on Unix, and fails the rename
/// on Windows. Leaving WAL mode requires zero other open handles (even idle
/// ones), and `BEGIN EXCLUSIVE` catches active rollback-mode transactions.
fn ensure_no_live_connections(db_path: &Path) -> Result<()> {
    if !db_path.exists() {
        return Ok(());
    }
    let Ok(conn) = crate::storage::db::open(db_path) else {
        // Unopenable file can't hold a SQLite connection — don't block the
        // restore-over-broken-DB use case; the pre-restore snapshot still guards data.
        return Ok(());
    };
    let _ = conn.busy_timeout(std::time::Duration::ZERO); // fail fast, never wait on the lock
    let probe = conn
        .query_row("PRAGMA journal_mode=DELETE", [], |_| Ok(()))
        .and_then(|()| conn.execute_batch("BEGIN EXCLUSIVE; COMMIT;"));
    match probe {
        Err(e) if is_busy(&e) => Err(CoreError::Conflict(
            "database is in use by another linXiv process (app, CLI, or MCP server) — \
             close it and retry"
                .to_string(),
        )),
        // ponytail: TOCTOU window between probe and rename; non-busy errors allowed
        // (broken/corrupted DB is what restore fixes). Upgrade if per-file locking added.
        Err(_) => Ok(()),
        Ok(_) => Ok(()),
    }
}

/// Validate that `src` is a readable, non-empty SQLite DB with a PAPER table.
pub fn validate_backup_source(src: &Path) -> Result<()> {
    let src_len = std::fs::metadata(src)
        .map_err(|e| CoreError::BadRequest(format!("cannot read backup file: {e}")))?
        .len();
    if src_len == 0 {
        return Err(CoreError::BadRequest(format!(
            "backup file is empty: {}",
            src.display()
        )));
    }

    // Validate first: open + query PAPER forces the header read and confirms
    // this is actually a linXiv DB.
    let check = Connection::open_with_flags(src, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| CoreError::BadRequest(format!("cannot read backup file: {e}")))?;
    let is_linxiv_db: i64 = check
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='PAPER'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| CoreError::BadRequest(format!("not a valid SQLite database: {e}")))?;
    if is_linxiv_db != 1 {
        return Err(CoreError::BadRequest(
            "not a valid linXiv database: no PAPER table".to_string(),
        ));
    }
    drop(check);
    Ok(())
}

/// Replace the DB at `db_path` with the snapshot at `src`, after checking `src`
/// is a readable SQLite DB. Refuses (`Conflict`) while another SQLite handle
/// holds the DB — see `ensure_no_live_connections` for what the probe can and
/// cannot see. The probe also checkpoints a WAL-mode `db_path` into its main
/// file, so the pre-restore snapshot below is complete on its own, except when
/// `ensure_no_live_connections` cannot open `db_path` at all (its unopenable-file
/// fallback): in that case, the pre-restore snapshot is a plain byte copy.
pub fn restore(src: &Path, db_path: &Path) -> Result<()> {
    validate_backup_source(src)?;

    ensure_no_live_connections(db_path)?;

    // Snapshot the DB about to be overwritten, sidecars included so a
    // rollback-journal-mode DB stays crash-consistent even if the later rename
    // fails. Snapshot failure aborts the restore — the user keeps a fallback.
    if db_path.exists() {
        let pre_restore = db_path.with_extension("pre-restore");
        if pre_restore.exists() {
            tracing::warn!(
                "overwriting previous pre-restore snapshot at {}",
                pre_restore.display()
            );
        }
        std::fs::copy(db_path, &pre_restore).map_err(|e| {
            CoreError::Internal(format!(
                "could not snapshot the existing database before restore \
                 (nothing was changed): {e}"
            ))
        })?;
        for suffix in ["-journal", "-wal", "-shm"] {
            let mut src_sidecar = db_path.as_os_str().to_owned();
            src_sidecar.push(suffix);
            let src_sidecar_path = PathBuf::from(&src_sidecar);
            if src_sidecar_path.exists() {
                let mut dst_sidecar = pre_restore.as_os_str().to_owned();
                dst_sidecar.push(suffix);
                std::fs::copy(&src_sidecar_path, PathBuf::from(dst_sidecar)).map_err(|e| {
                    CoreError::Internal(format!(
                        "could not snapshot sidecar {suffix} before restore \
                         (nothing was changed): {e}"
                    ))
                })?;
            }
        }
    }

    // Copy to a temp file in the same dir, then rename over db_path.
    let tmp = db_path.with_extension("restore.tmp");
    if let Err(e) = std::fs::copy(src, &tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(CoreError::Internal(format!("restore copy failed: {e}")));
    }

    // Drop stale sidecars from the pre-restore DB before the fresh file lands.
    for suffix in ["-journal", "-wal", "-shm"] {
        let mut sidecar = db_path.as_os_str().to_owned();
        sidecar.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(sidecar));
    }

    if let Err(e) = std::fs::rename(&tmp, db_path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(CoreError::Internal(format!("restore rename failed: {e}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_snapshots_a_valid_db_with_the_row() {
        let conn = crate::storage::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (v TEXT); INSERT INTO t (v) VALUES ('hi');")
            .unwrap();

        let dir = std::env::temp_dir().join(format!("linxiv-backup-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("snapshot.db");
        let _ = std::fs::remove_file(&dest); // VACUUM INTO refuses an existing file

        let info = backup(&conn, &dest).unwrap();
        assert_eq!(info.path, dest);
        assert!(info.bytes > 0);
        assert!(dest.exists());

        // Reopen the snapshot as its own DB and confirm the row survived.
        let restored = Connection::open(&dest).unwrap();
        let v: String = restored
            .query_row("SELECT v FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, "hi");

        // restore rejects a non-DB file.
        let junk = dir.join("junk.db");
        std::fs::write(&junk, b"not a database").unwrap();
        assert!(restore(&junk, &dest).is_err());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A file-based SQLite DB with a `PAPER` table, the minimal shape `restore`
    /// accepts as "a real linXiv DB".
    fn make_linxiv_like_db(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE PAPER (id INTEGER PRIMARY KEY); INSERT INTO PAPER (id) VALUES (1);",
        )
        .unwrap();
    }

    #[test]
    fn restore_rejects_empty_file() {
        let dir =
            std::env::temp_dir().join(format!("linxiv-backup-test-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("dest.db");
        std::fs::write(&dest, b"original contents").unwrap();

        let empty = dir.join("empty.db");
        std::fs::write(&empty, b"").unwrap();

        assert!(restore(&empty, &dest).is_err());
        assert_eq!(std::fs::read(&dest).unwrap(), b"original contents");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn restore_writes_pre_restore_snapshot_of_prior_db() {
        let dir = std::env::temp_dir().join(format!(
            "linxiv-backup-test-prerestore-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("dest.db");
        std::fs::write(&dest, b"prior contents").unwrap();

        let src = dir.join("src.db");
        make_linxiv_like_db(&src);

        restore(&src, &dest).unwrap();

        let pre_restore = dest.with_extension("pre-restore");
        assert_eq!(std::fs::read(&pre_restore).unwrap(), b"prior contents");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn restore_removes_stale_sidecar_files() {
        let dir = std::env::temp_dir().join(format!(
            "linxiv-backup-test-sidecars-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("dest.db");
        std::fs::write(&dest, b"prior contents").unwrap();
        for suffix in ["-journal", "-wal", "-shm"] {
            let mut sidecar = dest.as_os_str().to_owned();
            sidecar.push(suffix);
            std::fs::write(PathBuf::from(sidecar), b"stale").unwrap();
        }

        let src = dir.join("src.db");
        make_linxiv_like_db(&src);

        restore(&src, &dest).unwrap();

        for suffix in ["-journal", "-wal", "-shm"] {
            let mut sidecar = dest.as_os_str().to_owned();
            sidecar.push(suffix);
            assert!(!PathBuf::from(sidecar).exists());
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn backup_refuses_to_overwrite_existing_dest_with_clear_error() {
        let conn = crate::storage::open_in_memory().unwrap();
        let dir =
            std::env::temp_dir().join(format!("linxiv-backup-test-exists-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("snapshot.db");
        std::fs::write(&dest, b"already here").unwrap();

        let err = backup(&conn, &dest).unwrap_err();
        assert!(matches!(err, CoreError::BadRequest(_)));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn restore_refuses_while_wal_handle_open_then_succeeds_after_close() {
        let dir = std::env::temp_dir().join(format!(
            "linxiv-backup-test-wal-guard-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("dest.db");
        // A WAL-mode DB: any open handle (even idle) blocks leaving WAL mode.
        let live = Connection::open(&dest).unwrap();
        live.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))
            .unwrap();
        live.execute_batch("CREATE TABLE t (v TEXT); INSERT INTO t (v) VALUES ('old');")
            .unwrap();

        let src = dir.join("src.db");
        make_linxiv_like_db(&src);

        let err = restore(&src, &dest).unwrap_err();
        assert!(matches!(err, CoreError::Conflict(_)), "got: {err:?}");
        // Live handle still works
        let v: String = live.query_row("SELECT v FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(v, "old");

        drop(live);
        restore(&src, &dest).unwrap();
        let restored = Connection::open(&dest).unwrap();
        let n: i64 = restored
            .query_row("SELECT count(*) FROM PAPER", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn restore_refuses_during_active_rollback_transaction() {
        let dir = std::env::temp_dir().join(format!(
            "linxiv-backup-test-txn-guard-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("dest.db");
        make_linxiv_like_db(&dest);
        let live = Connection::open(&dest).unwrap();
        live.execute_batch("BEGIN IMMEDIATE;").unwrap(); // holds RESERVED

        let src = dir.join("src.db");
        make_linxiv_like_db(&src);

        let err = restore(&src, &dest).unwrap_err();
        assert!(matches!(err, CoreError::Conflict(_)), "got: {err:?}");

        live.execute_batch("COMMIT;").unwrap();
        drop(live);
        restore(&src, &dest).unwrap();

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn restore_over_corrupted_existing_db_succeeds() {
        let dir =
            std::env::temp_dir().join(format!("linxiv-backup-test-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Corrupted "DB": valid-looking size, garbage bytes — the live-connection
        // probe errors non-busy on it and must not block the restore.
        let dest = dir.join("dest.db");
        std::fs::write(&dest, vec![0xAB; 4096]).unwrap();

        let src = dir.join("src.db");
        make_linxiv_like_db(&src);

        restore(&src, &dest).unwrap();
        let restored = Connection::open(&dest).unwrap();
        let n: i64 = restored
            .query_row("SELECT count(*) FROM PAPER", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        // The corrupted original is preserved as the fallback snapshot.
        assert_eq!(
            std::fs::read(dest.with_extension("pre-restore")).unwrap(),
            vec![0xAB; 4096]
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn restore_rejects_missing_src_and_leaves_dest_untouched() {
        let dir =
            std::env::temp_dir().join(format!("linxiv-backup-test-missing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("dest.db");
        std::fs::write(&dest, b"original contents").unwrap();

        let missing = dir.join("does-not-exist.db");
        assert!(restore(&missing, &dest).is_err());
        assert_eq!(std::fs::read(&dest).unwrap(), b"original contents");

        std::fs::remove_dir_all(&dir).ok();
    }
}
