//! In-process backend state: the single SQLite connection behind a `Mutex` plus
//! the managed PDF/vault roots. Every router arm reaches the DB via `with_conn`.

use std::path::PathBuf;
use std::sync::Mutex;

use rusqlite::Connection;

use linxiv_core::{config, storage};

pub struct AppState {
    conn: Mutex<Connection>,
    /// Managed PDF directory (`config::pdf_dir()`), used by the PDF/export arms.
    pub pdf_dir: PathBuf,
    /// Obsidian vault root (`config::vault_dir()`), used by the export arms.
    pub vault_root: PathBuf,
}

impl AppState {
    /// Resolve + create the data dir, open the DB, run schema init. The data dir
    /// byte-matches Tauri's `app_data_dir()` for `com.linxiv.app` (D24).
    pub fn new() -> anyhow::Result<Self> {
        config::init_data_dir()?;
        let conn = storage::open(&config::db_path())?;
        storage::init_db(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            pdf_dir: config::pdf_dir(),
            vault_root: config::vault_dir(),
        })
    }

    /// Build from already-resolved parts — the DI seam for tests. Not `cfg(test)`
    /// because downstream crates' tests (linxiv-app) build through it too.
    pub fn from_parts(conn: Connection, pdf_dir: PathBuf, vault_root: PathBuf) -> Self {
        Self {
            conn: Mutex::new(conn),
            pdf_dir,
            vault_root,
        }
    }

    /// Locks the shared connection for the duration of `f`. Never hold the guard
    /// across an `.await`: `f` runs to completion and releases the lock first.
    /// A poisoned mutex is recovered, not propagated — a `Connection` has no broken
    /// invariant to protect, and refusing the lock forever would take every
    /// DB-touching route down for the rest of the process.
    ///
    /// TODO: Revisit for HUB roles — maybe parallel reads, serial writes.
    pub fn with_conn<T>(&self, f: impl FnOnce(&mut Connection) -> T) -> T {
        let mut guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    }
}
