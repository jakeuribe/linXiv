//! `linxiv-migrate` — forward-only schema migrator for the managed `papers.db`.
//! App/CLI/MCP already run `init_db` on open; this is the explicit scriptable step.

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
    println!(
        "linxiv-migrate: schema up to date at {} (version {version})",
        path.display()
    );
    Ok(())
}
