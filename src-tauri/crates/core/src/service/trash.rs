//! Trash-listing envelope — the one serialization of `GET /api/trash`,
//! `linxiv trash list`, and MCP `list_trash`.
//!
//! The rows are hand-picked projections: `DeletedPaperDetails` and
//! `ProjectDetails` carry more than the trash listing exposes (`pdf_path`,
//! `project_fks`, tags, …), so the named row types here are the wire contract.

use chrono::{NaiveDate, NaiveDateTime};
use rusqlite::Connection;
use serde::Serialize;

use crate::error::Result;
use crate::models::ProjectDetails;
use crate::service::paper::{self as svc_paper, DeletedPaperDetails};
use crate::service::project as svc_project;

/// One soft-deleted paper as the trash listing shows it.
#[derive(Debug, Serialize)]
pub struct TrashedPaperRow {
    pub source_fk: i64,
    pub source_id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub published: Option<NaiveDate>,
    pub deleted_at: Option<NaiveDateTime>,
    pub had_pdf: bool,
}

impl From<DeletedPaperDetails> for TrashedPaperRow {
    fn from(d: DeletedPaperDetails) -> Self {
        TrashedPaperRow {
            source_fk: d.source_fk,
            source_id: d.source_id,
            title: d.title,
            authors: d.authors,
            published: d.published,
            deleted_at: d.deleted_at,
            had_pdf: d.had_pdf,
        }
    }
}

/// One soft-deleted project as the trash listing shows it.
#[derive(Debug, Serialize)]
pub struct TrashedProjectRow {
    pub id: Option<i64>,
    pub name: String,
    /// `archived_at` is overwritten by delete(), so it holds the deletion time.
    pub deleted_at: Option<NaiveDateTime>,
    pub paper_count: usize,
}

impl From<ProjectDetails> for TrashedProjectRow {
    fn from(p: ProjectDetails) -> Self {
        TrashedProjectRow {
            id: p.id,
            name: p.name,
            deleted_at: p.archived_at,
            paper_count: p.source_fks.len(),
        }
    }
}

/// `{"papers": [...], "projects": [...]}` — newest-trashed first on both.
#[derive(Debug, Serialize)]
pub struct TrashListing {
    pub papers: Vec<TrashedPaperRow>,
    pub projects: Vec<TrashedProjectRow>,
}

/// Everything currently in the trash, in the canonical listing shape.
pub fn list_trash(conn: &Connection) -> Result<TrashListing> {
    Ok(TrashListing {
        papers: svc_paper::list_deleted(conn)?
            .into_iter()
            .map(Into::into)
            .collect(),
        projects: svc_project::list_deleted(conn)?
            .into_iter()
            .map(Into::into)
            .collect(),
    })
}

/// Receipt for a paper restore — one shape on route/CLI/MCP.
#[derive(Debug, Serialize)]
pub struct RestoredPaper {
    pub ok: bool,
    pub restored: String,
    pub pdf_path: Option<String>,
    pub project_fks: Vec<i64>,
}

/// Receipt for a paper hard-delete.
#[derive(Debug, Serialize)]
pub struct HardDeletedPaper {
    pub ok: bool,
    pub hard_deleted: String,
}

/// Receipt for a project restore.
#[derive(Debug, Serialize)]
pub struct RestoredProject {
    pub ok: bool,
    pub restored_project_id: i64,
}

/// Receipt for a project hard-delete.
#[derive(Debug, Serialize)]
pub struct HardDeletedProject {
    pub ok: bool,
    pub hard_deleted_project_id: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wire-shape pin: exact keys of the four mutation receipts.
    #[test]
    fn trash_receipt_wire_shapes() {
        assert_eq!(
            serde_json::to_string(&RestoredPaper {
                ok: true,
                restored: "arxiv:1".into(),
                pdf_path: None,
                project_fks: vec![2],
            })
            .unwrap(),
            r#"{"ok":true,"restored":"arxiv:1","pdf_path":null,"project_fks":[2]}"#
        );
        assert_eq!(
            serde_json::to_string(&HardDeletedPaper {
                ok: true,
                hard_deleted: "arxiv:1".into(),
            })
            .unwrap(),
            r#"{"ok":true,"hard_deleted":"arxiv:1"}"#
        );
        assert_eq!(
            serde_json::to_string(&RestoredProject {
                ok: true,
                restored_project_id: 5,
            })
            .unwrap(),
            r#"{"ok":true,"restored_project_id":5}"#
        );
        assert_eq!(
            serde_json::to_string(&HardDeletedProject {
                ok: true,
                hard_deleted_project_id: 5,
            })
            .unwrap(),
            r#"{"ok":true,"hard_deleted_project_id":5}"#
        );
    }

    /// Wire-shape pin: exact keys of both row types and the envelope.
    #[test]
    fn trash_listing_wire_shape() {
        let listing = TrashListing {
            papers: vec![TrashedPaperRow {
                source_fk: 1,
                source_id: "arxiv:1".into(),
                title: "T".into(),
                authors: vec!["A".into()],
                published: None,
                deleted_at: None,
                had_pdf: true,
            }],
            projects: vec![TrashedProjectRow {
                id: Some(5),
                name: "P".into(),
                deleted_at: None,
                paper_count: 2,
            }],
        };
        assert_eq!(
            serde_json::to_string(&listing).unwrap(),
            r#"{"papers":[{"source_fk":1,"source_id":"arxiv:1","title":"T","authors":["A"],"published":null,"deleted_at":null,"had_pdf":true}],"projects":[{"id":5,"name":"P","deleted_at":null,"paper_count":2}]}"#
        );
    }
}
