//! Storage layer — SQLite primitives + the per-entity named queries.
//! Plan §5.3 + D4/D5/D6.

pub mod backup;
pub mod db;
pub mod migrations;
pub mod queries;
pub mod query;
pub mod schema;

pub use backup::{backup, restore, validate_backup_source};
pub use db::{open, open_in_memory};
pub use queries::*;

use std::fs::File;

use rusqlite::Connection;

use crate::error::Result;

/// Exclusive advisory lock on a `<db>.init.lock` sidecar, serializing `init_db`
/// across processes (app/CLI/MCP cold-start one file) — `busy_timeout` covers
/// row writes, not a racer mid-DDL. An flock, NOT `BEGIN IMMEDIATE`: migrations
/// can't nest inside one transaction (some carry their own BEGIN/COMMIT).
/// `None` for in-memory/temp DBs; released when the returned `File` drops.
fn init_lock(conn: &Connection) -> Result<Option<File>> {
    let Some(db_path) = conn.path().filter(|p| !p.is_empty()) else {
        return Ok(None);
    };
    let mut lock_path = std::ffi::OsString::from(db_path);
    lock_path.push(".init.lock");
    let file = File::create(&lock_path)?;
    file.lock()?;
    Ok(Some(file))
}

/// Full startup init, FK-safe order: pre-schema dedup → tables → idempotent
/// migrations → views. Migrations MUST precede views (views select columns the
/// migrations add); the PROJECT_TO_PAPER dedup MUST precede tables (after
/// PAPER_TO_READING's composite FK exists its DML fails with "foreign key
/// mismatch"). The whole run holds the cross-process `init_lock` above.
pub fn init_db(conn: &Connection) -> Result<()> {
    let _lock = init_lock(conn)?;
    migrations::dedup_project_to_paper(conn)?;
    schema::apply_tables(conn)?;
    migrations::run_migrations(conn)?;
    schema::apply_views(conn)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: 8 threads, each with its OWN file-backed connection to one
    /// shared DB file, race the full init path — separate connections exercise
    /// the same SQLite-level DDL race as separate cold-start processes. Without
    /// the init lock this fails nondeterministically ("foreign key mismatch",
    /// "vtable constructor failed", "trigger ... already exists"), so several
    /// rounds on fresh files make the failure reliable.
    #[test]
    fn concurrent_init_from_separate_connections_is_serialized() {
        let dir = std::env::temp_dir().join(format!("linxiv-init-race-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for round in 0..10 {
            let path = dir.join(format!("race-{round}.db"));
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    let path = path.clone();
                    let barrier = barrier.clone();
                    std::thread::spawn(move || -> Result<()> {
                        let conn = open(&path)?;
                        barrier.wait();
                        init_db(&conn)
                    })
                })
                .collect();
            for h in handles {
                h.join().unwrap().unwrap();
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
