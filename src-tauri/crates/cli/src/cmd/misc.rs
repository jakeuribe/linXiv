//! Group `misc` — the flat top-level commands `stats`, `categories`, `settings`.
//! cmd_stats / cmd_categories / cmd_settings_* in `linxiv_cli.py`.

use std::path::PathBuf;

use clap::Subcommand;
use serde_json::{json, Value};

use linxiv_core::service::paper as svc_paper;
use linxiv_core::service::tag as svc_tag;
use linxiv_core::storage;

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

#[derive(Subcommand)]
pub enum MiscCmd {
    Stats,
    Categories,
    Settings {
        #[command(subcommand)]
        cmd: SettingsCmd,
    },
    /// Snapshot the database to a backup file.
    Backup {
        dest: PathBuf,
    },
}

pub async fn run(cmd: MiscCmd, ctx: &mut Ctx) -> anyhow::Result<()> {
    match cmd {
        // cmd_stats
        MiscCmd::Stats => {
            let papers = svc_paper::list_papers(&ctx.conn, true, None, 0, None)?;
            let categories = svc_paper::get_categories(&ctx.conn)?;
            let all_tags = svc_tag::list_all_tags(&ctx.conn)?;
            let pdf_count = papers.iter().filter(|p| p.has_pdf).count();
            output(&json!({
                "paper_count": papers.len(),
                "tag_count": all_tags.len(),
                "category_count": categories.len(),
                "pdf_count": pdf_count,
            }));
        }
        // cmd_categories
        MiscCmd::Categories => {
            output(&svc_paper::get_categories(&ctx.conn)?);
        }
        // cmd_settings_get
        MiscCmd::Settings {
            cmd: SettingsCmd::Get,
        } => {
            output(&ctx.settings.all());
        }
        // cmd_settings_update: JSON-parse the value, else keep it as a string.
        MiscCmd::Settings {
            cmd: SettingsCmd::Update { key, value },
        } => {
            let parsed: Value =
                serde_json::from_str(&value).unwrap_or_else(|_| Value::String(value.clone()));
            ctx.settings.set(key.clone(), parsed.clone())?;
            output(&json!({ key: parsed }));
        }
        MiscCmd::Backup { dest } => {
            output(&storage::backup(&ctx.conn, &dest)?);
        }
    }
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
        run(MiscCmd::Backup { dest: dest.clone() }, &mut ctx)
            .await
            .unwrap();
        assert!(dest.exists());

        // Simulate a corrupted/unreadable live DB: drop the handle, then clobber the file.
        let db_path = config::db_path();
        drop(ctx);
        std::fs::write(&db_path, b"not a database").unwrap();

        storage::restore(&dest, &db_path).unwrap();

        let restored = Connection::open(&db_path).unwrap();
        let v: String = restored
            .query_row("SELECT v FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, "hi");

        let _ = std::fs::remove_dir_all(&dir);
        env::remove_var("LINXIV_DATA_DIR");
    }
}
