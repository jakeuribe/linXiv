//! `linxiv-migrate` — forward-only schema migrator for the managed `papers.db`.
//!
//! Opens the database and runs `linxiv-core`'s idempotent migrations (tables +
//! numbered guards + views, via `storage::init_db`). This is a SCAFFOLD for FUTURE
//! schema changes: add them to `crates/core/src/storage/migrations.rs` and this
//! binary applies whatever is pending on the next run. It deliberately knows
//! nothing about the pre-Rust-port (legacy blue→green) schema — existing DBs
//! already match the current schema, so no one-time import is needed.
//!
//! The app/CLI/MCP also run `init_db` on every open, so migrations are normally
//! applied automatically; this binary exists for an explicit, scriptable
//! "apply pending migrations" step (e.g. before a release, or in CI).

use linxiv_core::{config, storage};

fn main() -> anyhow::Result<()> {
    config::init_data_dir()?;
    let path = config::db_path();
    let conn = storage::open(&path)?;
    storage::init_db(&conn)?;
    let version: String = conn
        .query_row(
            "SELECT VERSION FROM DB_VERSION ORDER BY VERSION_FK DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap_or_else(|_| "unknown".to_string());
    println!("linxiv-migrate: schema up to date at {} (version {version})", path.display());
    Ok(())
}
