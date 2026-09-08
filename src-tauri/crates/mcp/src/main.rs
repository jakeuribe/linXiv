//! linXiv MCP server — the library tools over stdio JSON-RPC. The 75 tools are
//! split across five cluster modules, each a `#[tool_router]` impl merged here.

mod annotations;
mod io_authors_misc;
mod notes_pdf_trash;
mod papers;
mod projects_tags;
mod util;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::transport::stdio;
use rmcp::{tool_handler, ServerHandler, ServiceExt};
use rusqlite::Connection;
use tracing_subscriber::EnvFilter;

use linxiv_core::config;
use linxiv_core::service::db_admin;

/// Shared MCP server state: the single SQLite connection (opened once, behind a
/// `Mutex`), the managed PDF root, and the merged tool router.
#[derive(Clone)]
pub struct Server {
    conn: Arc<Mutex<Connection>>,
    /// Managed PDF directory (`config::pdf_dir()`). Used by the PDF tools.
    pdf_dir: PathBuf,
    tool_router: ToolRouter<Self>,
}

impl Server {
    /// Open the data dir, the DB, and run startup init, then assemble the
    /// merged router.
    pub fn new() -> anyhow::Result<Self> {
        config::init_data_dir()?;
        let conn = db_admin::open_app_db()?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            pdf_dir: config::pdf_dir(),
            tool_router: Self::tools_papers()
                + Self::tools_projects_tags()
                + Self::tools_notes_pdf_trash()
                + Self::tools_annotations()
                + Self::tools_io_authors_misc(),
        })
    }

    /// Locks the shared connection for the duration of `f`. Panics only if the
    /// lock is poisoned (a previous tool body panicked while holding it).
    pub fn with_conn<T>(&self, f: impl FnOnce(&mut Connection) -> T) -> T {
        let mut guard = self.conn.lock().expect("db connection mutex poisoned");
        f(&mut guard)
    }

    /// The shared handle for work that must run off the async runtime: `with_conn`
    /// blocks a tokio worker, fine for fast statements but not whole-DB file I/O.
    pub fn conn_handle(&self) -> Arc<Mutex<Connection>> {
        Arc::clone(&self.conn)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("linXiv: search, fetch, organize, and annotate academic papers.")
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // stdout is the JSON-RPC channel; all logs must go to stderr.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let server = Server::new()?;
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
