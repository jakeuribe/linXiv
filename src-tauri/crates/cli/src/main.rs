//! linXiv headless CLI — Rust port of `linxiv_cli.py`.
//!
//! Shared skeleton: the top-level clap tree + the group module contract. Command
//! bodies live in `cmd::<group>::run` (still `todo!()` here); this file only wires
//! parsing → lazy `Ctx::open()` → dispatch. Output/error JSON parity helpers are in
//! `output`, the DB/data-dir seam in `ctx`.

mod cmd;
mod ctx;
mod output;

use clap::{Parser, Subcommand};

use ctx::Ctx;

#[derive(Parser)]
#[command(name = "linxiv", version, about = "linXiv headless CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// All 15 top-level groups. `search`/`fetch`/`list` are flat commands routed into
/// the `library` group; `stats`/`categories`/`settings` route into `misc`.
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
        Commands::Pdf { cmd } => cmd::pdf::run(cmd, ctx).await,
        Commands::Trash { cmd } => cmd::trash::run(cmd, ctx).await,
        Commands::Doi { cmd } => cmd::doi::run(cmd, ctx).await,
        Commands::Author { cmd } => cmd::author::run(cmd, ctx).await,
        Commands::Bibtex { cmd } => cmd::bibtex::run(cmd, ctx).await,
        Commands::Stats => cmd::misc::run(MiscCmd::Stats, ctx).await,
        Commands::Categories => cmd::misc::run(MiscCmd::Categories, ctx).await,
        Commands::Settings { cmd } => cmd::misc::run(MiscCmd::Settings { cmd }, ctx).await,
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    // Lazy: open the DB/data-dir once before dispatch (cheap, no network).
    let mut ctx = Ctx::open().unwrap_or_else(|e| output::fail(e));
    if let Err(e) = dispatch(cli.command, &mut ctx).await {
        output::fail(e);
    }
}
