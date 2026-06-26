//! Group `note` — cmd_note_* in `linxiv_cli.py`.

use clap::Subcommand;
use serde::Serialize;

use linxiv_core::models::{NoteIn, NoteUpdateIn};
use linxiv_core::service::note as svc_note;
use linxiv_core::service::note::{Note, Notes};
use linxiv_core::service::project as svc_project;
use linxiv_core::service::project::Project;
use linxiv_core::storage::queries::paper as paper_q;

use crate::ctx::Ctx;
use crate::output::{as_source_id, fail, output};

#[derive(Subcommand)]
pub enum NoteCmd {
    /// Create a note on a paper
    Create {
        source_id: String,
        /// Note body text
        content: String,
        #[arg(long, default_value = "")]
        title: String,
        /// Associate note with a project
        #[arg(long = "project-id")]
        project_id: Option<i64>,
    },
    /// Get a note by ID
    Get { note_id: i64 },
    /// List notes
    List {
        /// Filter by paper source ID
        #[arg(long = "paper-id")]
        source_id: Option<String>,
        /// Filter by project ID
        #[arg(long = "project-id")]
        project_id: Option<i64>,
    },
    /// Update note title or content
    Update {
        note_id: i64,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        content: Option<String>,
    },
    /// Delete a note by ID
    Delete { note_id: i64 },
}

/// `cmd_note_create` output dict.
#[derive(Serialize)]
struct CreatedNote {
    id: i64,
    source_fk: i64,
    project_id: Option<i64>,
    title: String,
}

/// `cmd_note_update` output dict.
#[derive(Serialize)]
struct UpdatedNote {
    id: i64,
    updated: bool,
}

/// `cmd_note_delete` output dict.
#[derive(Serialize)]
struct DeletedNote {
    deleted_note_id: i64,
}

pub async fn run(cmd: NoteCmd, ctx: &mut Ctx) -> anyhow::Result<()> {
    let conn = &ctx.conn;
    match cmd {
        NoteCmd::Create { source_id, content, title, project_id } => {
            // Project existence is validated before paper resolution (Python order).
            if let Some(pid) = project_id {
                if svc_project::get(conn, &Project { project_fk: Some(pid) })?.is_none() {
                    fail(format!("Project {pid} not found"));
                }
            }
            let source_id = as_source_id(&source_id, "arxiv");
            let source_fk = match paper_q::get_paper_root(conn, &source_id)? {
                Some(root) => root.source_fk,
                None => fail(format!("Paper {source_id} not found in DB")),
            };
            let note_id = svc_note::create(
                conn,
                &NoteIn {
                    source_fk,
                    title: title.clone(),
                    content,
                    paper_id: None,
                    project_fk: project_id,
                },
            )?;
            output(&CreatedNote { id: note_id, source_fk, project_id, title });
        }
        NoteCmd::Get { note_id } => {
            match svc_note::get(conn, &Note { note_id: Some(note_id) })? {
                Some(details) => output(&details),
                None => fail(format!("Note {note_id} not found")),
            }
        }
        NoteCmd::List { source_id, project_id } => {
            let mut source_fk: Option<i64> = None;
            if let Some(raw) = source_id {
                let sid = as_source_id(&raw, "arxiv");
                match paper_q::get_paper_root(conn, &sid)? {
                    Some(root) => source_fk = Some(root.source_fk),
                    None => fail(format!("Paper {} not found in DB", crate::output::pyrepr(&sid))),
                }
            }
            let notes = if source_fk.is_none() && project_id.is_none() {
                svc_note::list_all(conn)?
            } else {
                svc_note::get_many(
                    conn,
                    &Notes { source_fk, project_fk: project_id, ..Default::default() },
                )?
            };
            output(&notes);
        }
        NoteCmd::Update { note_id, title, content } => {
            if svc_note::get(conn, &Note { note_id: Some(note_id) })?.is_none() {
                fail(format!("Note {note_id} not found"));
            }
            if title.is_none() && content.is_none() {
                fail("at least one of --title or --content must be provided");
            }
            svc_note::update(conn, &NoteUpdateIn { note_id, title, content })?;
            output(&UpdatedNote { id: note_id, updated: true });
        }
        NoteCmd::Delete { note_id } => {
            if svc_note::get(conn, &Note { note_id: Some(note_id) })?.is_none() {
                fail(format!("Note {note_id} not found"));
            }
            svc_note::delete(conn, &Note { note_id: Some(note_id) })?;
            output(&DeletedNote { deleted_note_id: note_id });
        }
    }
    Ok(())
}
