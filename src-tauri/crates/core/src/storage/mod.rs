//! Storage layer — SQLite primitives + the per-entity named queries.
//! Rust port of the `storage/` package. Plan §5.3 + D4/D5/D6.
//!
//! This stage lands the COMPILING primitives (db / query / schema / migrations);
//! the named queries below are signature-only stubs so the API surface exists.
//! Correctness of the primitives outranks covering every named query.

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
/// across processes (app, CLI, MCP sidecar all cold-start against one file).
/// Without it, two connections racing the DDL bootstrap crash with raw
/// SQLITE_ERRORs ("trigger ... already exists", "vtable constructor failed",
/// "no such table") — `busy_timeout` only covers row writes, not a racer mid-DDL.
///
/// An flock, NOT `BEGIN IMMEDIATE` around the init run: the migrations can't
/// nest inside one transaction (12_paper_to_reading_cascade_fk.sql has its own
/// BEGIN/COMMIT, `backfill_uuid_column` opens its own transaction). Returns
/// `None` for in-memory/temp DBs (`Connection::path()` is `None`/`""`) — those
/// are single-process by construction and have no dir to lock in. The lock is
/// released when the returned `File` drops.
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

/// Full first-run / startup init for a real user DB, in the FK-safe order
/// `storage/config/core.py::apply_sql_schema` uses: pre-schema dedup → tables →
/// idempotent migrations → views. Migrations MUST run before views: views select
/// columns (STATUS, PROVIDER) that the migrations add to legacy tables. The
/// PROJECT_TO_PAPER dedup MUST run before tables: once apply_tables creates
/// PAPER_TO_READING's composite FK, dedup DML on the unindexed parent key fails
/// with "foreign key mismatch" (see `migrations::dedup_project_to_paper`).
/// The whole run holds the cross-process `init_lock` above.
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
