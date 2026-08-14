//! Trash-listing envelope — the one serialization of `GET /api/trash`,
//! `linxiv trash list`, and MCP `list_trash`.
//!
//! Rows carry the FULL data: papers are `DeletedPaperDetails` as-is, projects
//! are the canonical `ProjectOut` plus an explicit `deleted_at`. The GUI
//! renders a subset; CLI/MCP consumers get everything (`pdf_path`,
//! `project_fks`, tags, …) — trimming here would drop functionality the
//! frontend merely doesn't surface yet.

use chrono::NaiveDateTime;
use rusqlite::Connection;
use serde::Serialize;

use crate::error::Result;
use crate::models::ProjectOut;
use crate::service::paper::{self as svc_paper, DeletedPaperDetails};
use crate::service::project as svc_project;

/// One soft-deleted project: the canonical `ProjectOut` fields (flattened)
/// plus the deletion time (`delete()` overwrites `archived_at` with it).
#[derive(Debug, Serialize)]
pub struct TrashedProjectRow {
    #[serde(flatten)]
    pub project: ProjectOut,
    pub deleted_at: Option<NaiveDateTime>,
}

/// `{"papers": [...], "projects": [...]}` — newest-trashed first on both.
#[derive(Debug, Serialize)]
pub struct TrashListing {
    pub papers: Vec<DeletedPaperDetails>,
    pub projects: Vec<TrashedProjectRow>,
}

/// Everything currently in the trash, in the canonical listing shape.
pub fn list_trash(conn: &Connection) -> Result<TrashListing> {
    Ok(TrashListing {
        papers: svc_paper::list_deleted(conn)?,
        projects: svc_project::list_deleted(conn)?
            .into_iter()
            .map(|p| {
                let deleted_at = p.archived_at;
                Ok(TrashedProjectRow {
                    project: svc_project::to_out(conn, p)?,
                    deleted_at,
                })
            })
            .collect::<Result<Vec<_>>>()?,
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
            papers: vec![DeletedPaperDetails {
                source_fk: 1,
                source_id: "arxiv:1".into(),
                title: "T".into(),
                authors: vec!["A".into()],
                published: None,
                deleted_at: None,
                pdf_path: None,
                had_pdf: true,
                project_fks: vec![5],
            }],
            projects: vec![TrashedProjectRow {
                project: crate::models::ProjectOut {
                    id: 5,
                    name: "P".into(),
                    description: String::new(),
                    color_hex: None,
                    project_tags: vec![],
                    source_ids: vec!["arxiv:1".into(), "arxiv:2".into()],
                    paper_count: 2,
                    status: crate::models::Status::Deleted,
                    created_at: None,
                    updated_at: None,
                    archived_at: None,
                    share_id: None,
                },
                deleted_at: None,
            }],
        };
        assert_eq!(
            serde_json::to_string(&listing).unwrap(),
            concat!(
                r#"{"papers":[{"source_fk":1,"source_id":"arxiv:1","title":"T","authors":["A"],"#,
                r#""published":null,"deleted_at":null,"pdf_path":null,"had_pdf":true,"project_fks":[5]}],"#,
                r#""projects":[{"id":5,"name":"P","description":"","color_hex":null,"project_tags":[],"#,
                r#""source_ids":["arxiv:1","arxiv:2"],"paper_count":2,"status":"deleted","created_at":null,"#,
                r#""updated_at":null,"archived_at":null,"share_id":null,"deleted_at":null}]}"#
            )
        );
    }
}
