//! The DB / data-dir seam. Opened once, lazily, in `main` before dispatch — every
//! group `run` borrows it `&mut` (the rusqlite `Connection` needs `&mut` for txns).

use std::path::PathBuf;

use anyhow::Result;
use rusqlite::Connection;

use linxiv_core::config;
use linxiv_core::service::db_admin;

/// Process-wide handles a command needs: the open DB plus the resolved data paths.
pub struct Ctx {
    pub conn: Connection,
    pub pdf_dir: PathBuf,
    pub settings: config::UserSettings,
}

impl Ctx {
    /// Resolve + create the data dir, open the DB, run schema init, and load
    /// settings. Mirrors `linxiv_cli.py::main` startup (init_data_dir → init_db).
    pub fn open() -> Result<Self> {
        config::init_data_dir()?;
        let conn = db_admin::open_app_db()?;
        let settings = config::UserSettings::load()?;
        Ok(Self {
            conn,
            pdf_dir: config::pdf_dir(),
            settings,
        })
    }
}
