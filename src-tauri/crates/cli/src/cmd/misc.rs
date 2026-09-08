//! Group `misc` — the flat top-level commands `stats`, `categories`, `settings`.

use std::path::PathBuf;

use clap::Subcommand;
use serde_json::json;

use linxiv_core::service::db_admin;
use linxiv_core::service::paper as svc_paper;

use crate::ctx::Ctx;
use crate::output::output;

#[derive(Subcommand)]
pub enum SettingsCmd {
    /// Show all current settings
    Get,
    /// Set a setting value
    Update {
        key: String,
        /// New value (JSON-parsed if valid JSON, else string)
        value: String,
    },
}

// cmd_stats — `service::stats` owns the envelope (ADR-0011: gained `recent_papers`).
pub async fn stats(ctx: &mut Ctx) -> anyhow::Result<()> {
    output(&linxiv_core::service::stats::stats(&ctx.conn)?);
    Ok(())
}

// cmd_categories
pub async fn categories(ctx: &mut Ctx) -> anyhow::Result<()> {
    output(&svc_paper::get_categories(&ctx.conn)?);
    Ok(())
}

// cmd_settings_get / cmd_settings_update (JSON-parse the value, else keep it as a string).
pub async fn settings(cmd: SettingsCmd, ctx: &mut Ctx) -> anyhow::Result<()> {
    match cmd {
        SettingsCmd::Get => output(&ctx.settings.all()),
        SettingsCmd::Update { key, value } => {
            let parsed = ctx.settings.set_from_str(key.clone(), value)?;
            output(&json!({ key: parsed }));
        }
    }
    Ok(())
}

pub async fn backup(dest: PathBuf, ctx: &mut Ctx) -> anyhow::Result<()> {
    output(&db_admin::backup(&ctx.conn, &dest)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use linxiv_core::config;
    use rusqlite::Connection;
    use std::env;

    // Single test: LINXIV_DATA_DIR is process-global, so keep everything sequential
    // (matches the convention in linxiv_core::config's own tests).
    #[tokio::test]
    async fn backup_then_restore_round_trips_even_if_live_db_is_corrupted() {
        let dir = env::temp_dir().join(format!("linxiv-cli-backup-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        env::set_var("LINXIV_DATA_DIR", &dir);

        let mut ctx = Ctx::open().unwrap();
        ctx.conn
            .execute_batch("CREATE TABLE t (v TEXT); INSERT INTO t (v) VALUES ('hi');")
            .unwrap();

        let dest = dir.join("snapshot.db");
        backup(dest.clone(), &mut ctx).await.unwrap();
        assert!(dest.exists());

        // Simulate a corrupted/unreadable live DB: drop the handle, then clobber the file.
        let db_path = config::db_path();
        drop(ctx);
        std::fs::write(&db_path, b"not a database").unwrap();

        db_admin::restore_closed(&dest).unwrap();

        let restored = Connection::open(&db_path).unwrap();
        let v: String = restored
            .query_row("SELECT v FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, "hi");

        let _ = std::fs::remove_dir_all(&dir);
        env::remove_var("LINXIV_DATA_DIR");
    }
}
