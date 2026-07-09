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

pub use backup::{backup, restore, validate_backup_source, BackupInfo};
pub use db::{open, open_in_memory, transaction};
pub use queries::*;
pub use query::{_in, Q};

use rusqlite::Connection;

use crate::error::Result;

/// Full first-run / startup init for a real user DB, in the FK-safe order
/// `storage/config/core.py::apply_sql_schema` uses: tables → idempotent
/// migrations → views. Migrations MUST run before views: views select columns
/// (STATUS, PROVIDER) that the migrations add to legacy tables.
pub fn init_db(conn: &Connection) -> Result<()> {
    schema::apply_tables(conn)?;
    migrations::run_migrations(conn)?;
    schema::apply_views(conn)?;
    Ok(())
}
