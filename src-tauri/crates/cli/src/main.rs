//! linXiv headless CLI — Rust port of `linxiv_cli.py`.
//!
//! Shared skeleton: the top-level clap tree + the group module contract. Command
//! bodies live in `cmd::<group>::run`; this file only wires parsing → lazy `Ctx::open()`
//! → dispatch. Output/error JSON parity helpers are in `output`, the DB/data-dir seam in `ctx`.

mod cmd;
mod ctx;
mod output;

use clap::{Parser, Subcommand};

use ctx::Ctx;
use linxiv_core::{config, storage};

#[derive(Parser)]
#[command(name = "linxiv", version, about = "linXiv headless CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// All 19 top-level groups. `search`/`fetch`/`list` are flat commands routed into
/// the `library` group; `stats`/`categories`/`settings`/`backup` route into `misc`.
/// `restore` and `pdf-meta` are special-cased in `main` before `Ctx::open()` so they
/// still work without requiring a valid DB; their dispatch arms exist for match exhaustiveness.
#[derive(Subcommand)]
enum Commands {
    /// Search for papers
    Search(cmd::library::SearchArgs),
    /// Fetch and save a paper by ID
    Fetch(cmd::library::FetchArgs),
    /// List papers in the database
    List(cmd::library::ListArgs),
    /// Manage individual papers
    Paper {
        #[command(subcommand)]
        cmd: cmd::paper::PaperCmd,
    },
    /// Manage tags
    Tag {
        #[command(subcommand)]
        cmd: cmd::tag::TagCmd,
    },
    /// Manage projects
    Project {
        #[command(subcommand)]
        cmd: cmd::project::ProjectCmd,
    },
    /// Manage notes
    Note {
        #[command(subcommand)]
        cmd: cmd::note::NoteCmd,
    },
    /// Manage PDF highlight annotations
    Annotation {
        #[command(subcommand)]
        cmd: cmd::annotation::AnnotationCmd,
    },
    /// Manage PDFs
    Pdf {
        #[command(subcommand)]
        cmd: cmd::pdf::PdfCmd,
    },
    /// Manage soft-deleted items
    Trash {
        #[command(subcommand)]
        cmd: cmd::trash::TrashCmd,
    },
    /// Resolve and save papers by DOI
    Doi {
        #[command(subcommand)]
        cmd: cmd::doi::DoiCmd,
    },
    /// Manage authors
    Author {
        #[command(subcommand)]
        cmd: cmd::author::AuthorCmd,
    },
    /// BibTeX import
    Bibtex {
        #[command(subcommand)]
        cmd: cmd::bibtex::BibtexCmd,
    },
    /// Library statistics
    Stats,
    /// List all paper categories in the library
    Categories,
    /// View and update user settings
    Settings {
        #[command(subcommand)]
        cmd: cmd::misc::SettingsCmd,
    },
    /// Snapshot the database to a backup file
    Backup { dest: std::path::PathBuf },
    /// Restore the database from a backup snapshot
    Restore { src: std::path::PathBuf },
    /// Hidden pdfium worker: extraction runs in this child process so a native
    /// libpdfium crash kills the child, not the app (core `pdf_metadata`).
    #[command(hide = true)]
    PdfMeta { path: std::path::PathBuf },
}

async fn dispatch(command: Commands, ctx: &mut Ctx) -> anyhow::Result<()> {
    use cmd::library::LibraryCmd;
    use cmd::misc::MiscCmd;
    match command {
        Commands::Search(a) => cmd::library::run(LibraryCmd::Search(a), ctx).await,
        Commands::Fetch(a) => cmd::library::run(LibraryCmd::Fetch(a), ctx).await,
        Commands::List(a) => cmd::library::run(LibraryCmd::List(a), ctx).await,
        Commands::Paper { cmd } => cmd::paper::run(cmd, ctx).await,
        Commands::Tag { cmd } => cmd::tag::run(cmd, ctx).await,
        Commands::Project { cmd } => cmd::project::run(cmd, ctx).await,
        Commands::Note { cmd } => cmd::note::run(cmd, ctx).await,
        Commands::Annotation { cmd } => cmd::annotation::run(cmd, ctx).await,
        Commands::Pdf { cmd } => cmd::pdf::run(cmd, ctx).await,
        Commands::Trash { cmd } => cmd::trash::run(cmd, ctx).await,
        Commands::Doi { cmd } => cmd::doi::run(cmd, ctx).await,
        Commands::Author { cmd } => cmd::author::run(cmd, ctx).await,
        Commands::Bibtex { cmd } => cmd::bibtex::run(cmd, ctx).await,
        Commands::Stats => cmd::misc::run(MiscCmd::Stats, ctx).await,
        Commands::Categories => cmd::misc::run(MiscCmd::Categories, ctx).await,
        Commands::Settings { cmd } => cmd::misc::run(MiscCmd::Settings { cmd }, ctx).await,
        Commands::Backup { dest } => cmd::misc::run(MiscCmd::Backup { dest }, ctx).await,
        // `main` intercepts Restore and PdfMeta before Ctx::open()
        // (see above); these arms exist only for match exhaustiveness.
        Commands::Restore { .. } => unreachable!("restore is handled in main() before dispatch"),
        Commands::PdfMeta { .. } => unreachable!("pdf-meta is handled in main() before dispatch"),
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        // Bypasses Ctx::open(): the worker must not touch the DB (no contention
        // with the parent) and must stay silent on stdout except the JSON record.
        // ponytail: compact JSON (direct println, no output::output) is intentional — worker IPC only, not user-facing CLI contract.
        Commands::PdfMeta { path } => {
            let bytes = std::fs::read(&path).unwrap_or_else(|e| output::fail(e));
            println!(
                "{}",
                tokio::task::spawn_blocking(move || {
                    linxiv_core::sources::pdf_metadata::extract_pdf_metadata_json(&bytes)
                })
                .await
                .unwrap_or_else(|e| output::fail(e))
            );
        }
        // Bypasses Ctx::open()/init_db so restore works even on a broken DB.
        Commands::Restore { src } => {
            let db_path = config::db_path();
            match storage::restore(&src, &db_path) {
                Ok(()) => {
                    output::output(&serde_json::json!({ "restored": db_path.to_string_lossy() }))
                }
                Err(e) => output::fail(e),
            }
        }
        command => {
            // Lazy: open the DB/data-dir once before dispatch (cheap, no network).
            let mut ctx = Ctx::open().unwrap_or_else(|e| output::fail(e));
            if let Err(e) = dispatch(command, &mut ctx).await {
                output::fail(e);
            }
        }
    }
}
