//! Group `misc` — the flat top-level commands `stats`, `categories`, `settings`.
//! cmd_stats / cmd_categories / cmd_settings_* in `linxiv_cli.py`.

use clap::Subcommand;
use serde_json::{json, Value};

use linxiv_core::service::paper as svc_paper;
use linxiv_core::service::tag as svc_tag;

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
    }
    Ok(())
}
