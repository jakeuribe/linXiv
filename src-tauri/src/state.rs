//! In-process backend state. Holds the single SQLite connection (guarded by a
//! `Mutex`, opened once at startup) plus the managed PDF/vault roots — the app's
//! analogue of `linxiv-mcp`'s `Server` and `linxiv-cli`'s `Ctx`. Every router arm
//! reaches the DB through `with_conn`. Replaces the HTTP hop to the Python sidecar.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

use linxiv_core::{config, storage};

pub struct AppState {
    conn: Arc<Mutex<Connection>>,
    /// Managed PDF directory (`config::pdf_dir()`), used by the PDF/export arms.
    pub pdf_dir: PathBuf,
    /// Obsidian vault root (`config::vault_dir()`), used by the export arms.
    pub vault_root: PathBuf,
}

impl AppState {
    /// Resolve + create the data dir, open the DB, and run schema init. Mirrors
    /// `Ctx::open()` / `Server::new()`. `config`'s data dir byte-matches Tauri's
    /// `app_data_dir()` for `com.linxiv.app` (D24), so this lands on the same
    /// `papers.db` the packaged app uses.
    pub fn new() -> anyhow::Result<Self> {
        config::init_data_dir()?;
        let conn = storage::open(&config::db_path())?;
        storage::init_db(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            pdf_dir: config::pdf_dir(),
            vault_root: config::vault_dir(),
        })
    }

    /// Build from already-resolved parts. The DI seam for tests: pass an
    /// `open_in_memory()` connection and tempdirs instead of touching the real
    /// data dir (mirrors how the core/cli/mcp tests isolate).
    pub fn from_parts(conn: Connection, pdf_dir: PathBuf, vault_root: PathBuf) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
            pdf_dir,
            vault_root,
        }
    }

    /// The one accessor every router arm uses to reach the DB. Locks the shared
    /// connection for the duration of `f`. Never hold the guard across an `.await`:
    /// `f` runs to completion and releases the lock before the caller awaits.
    pub fn with_conn<T>(&self, f: impl FnOnce(&mut Connection) -> T) -> T {
        let mut guard = self.conn.lock().expect("db connection mutex poisoned");
        f(&mut guard)
    }
}
