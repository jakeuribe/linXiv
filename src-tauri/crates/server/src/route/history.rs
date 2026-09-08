//! `/api/history` routes — git-style change log, per-change diff, and restore
//! over the journal docs (projects + library) and share docs (log/diff only;
//! restoring a shared project goes through its journal doc, and the next
//! hoster sync pushes the restored state outward).

use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;

use linxiv_share::{
    device_actor_hex, doc_history, e2ee_dir, e2ee_received_dir, received_dir, snapshot_at,
    valid_share_id, RemovalOutcome, SharedProject,
};

use crate::journal;
use crate::route::{path_i64, ApiError, ReqCtx};
use crate::state::AppState;

pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("GET", ["api", "history", "library"]) => {
            Some(timeline(&journal::journal_dir(), journal::LIBRARY_DOC))
        }
        ("GET", ["api", "history", "library", "diff"]) => Some(change_diff(
            &journal::journal_dir(),
            journal::LIBRARY_DOC,
            ctx,
        )),
        ("POST", ["api", "history", "library", "restore"]) => Some(restore_library(state, ctx)),
        ("GET", ["api", "history", "project", fk]) => Some(
            path_i64(fk)
                .and_then(|fk| timeline(&journal::journal_dir(), &journal::project_doc_id(fk))),
        ),
        ("GET", ["api", "history", "project", fk, "diff"]) => Some(path_i64(fk).and_then(|fk| {
            change_diff(&journal::journal_dir(), &journal::project_doc_id(fk), ctx)
        })),
        ("POST", ["api", "history", "project", fk, "restore"]) => {
            Some(path_i64(fk).and_then(|fk| restore_project(state, fk, ctx)))
        }
        ("GET", ["api", "history", "share", id]) => {
            Some(share_doc_dir(id).and_then(|d| timeline(&d, id)))
        }
        ("GET", ["api", "history", "share", id, "diff"]) => {
            Some(share_doc_dir(id).and_then(|d| change_diff(&d, id, ctx)))
        }
        _ => None,
    }
}

/// One row of `GET /api/history/...` (`HistoryChange` in src/api/history.ts).
#[derive(Serialize, ts_rs::TS)]
pub struct ChangeRow {
    pub hash: String,
    pub actor: String,
    /// Unix seconds; 0 on changes written before timestamps landed.
    pub time: i64,
    pub message: Option<String>,
    /// Written by this device (its pinned actor id).
    pub mine: bool,
}

/// `GET /api/history/{scope}` envelope: oldest-first change log + our actor.
#[derive(Serialize, ts_rs::TS)]
pub struct Timeline {
    pub changes: Vec<ChangeRow>,
    pub device_actor: Option<String>,
}

fn timeline(dir: &std::path::Path, doc_id: &str) -> Result<Value, ApiError> {
    let device = device_actor_hex();
    let log = match doc_history(dir, doc_id) {
        Ok(log) => log,
        // No doc yet (journal pass pending, or a pre-first-sync e2ee
        // placeholder): an empty log, so the UI shows "No history yet".
        Err(linxiv_share::ShareError::NotFound(_)) => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    let changes: Vec<ChangeRow> = log
        .into_iter()
        .map(|c| ChangeRow {
            mine: Some(c.actor.as_str()) == device.as_deref(),
            hash: c.hash,
            actor: c.actor,
            time: c.time,
            message: c.message,
        })
        .collect();
    crate::route::to_value(&Timeline {
        changes,
        device_actor: device,
    })
}

#[derive(Serialize, ts_rs::TS)]
pub struct PaperChange {
    pub source_id: String,
    pub title: String,
}

/// Added/removed carry no body; `changed` rows carry truncated from→to text.
#[derive(Serialize, ts_rs::TS)]
pub struct EntryChange {
    pub uuid: String,
    pub title: String,
    pub from: Option<String>,
    pub to: Option<String>,
}

#[derive(Serialize, ts_rs::TS)]
pub struct FieldChange {
    pub field: String,
    pub from: String,
    pub to: String,
}

/// `GET /api/history/{scope}/diff?at=<hash>` — what that one change did,
/// git-show style: state at its deps vs state at the change.
#[derive(Serialize, Default, ts_rs::TS)]
pub struct HistoryDiff {
    pub papers_added: Vec<PaperChange>,
    pub papers_removed: Vec<PaperChange>,
    pub tags_added: Vec<String>,
    pub tags_removed: Vec<String>,
    pub notes_added: Vec<EntryChange>,
    pub notes_removed: Vec<EntryChange>,
    pub notes_changed: Vec<EntryChange>,
    pub annotations_added: Vec<EntryChange>,
    pub annotations_removed: Vec<EntryChange>,
    pub annotations_changed: Vec<EntryChange>,
    pub meta: Vec<FieldChange>,
}

fn change_diff(dir: &std::path::Path, doc_id: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let at = ctx
        .q("at")
        .ok_or_else(|| ApiError::new(422, "missing ?at=<change hash>"))?;
    let history = doc_history(dir, doc_id)?;
    let change = history
        .iter()
        .find(|c| c.hash == at)
        .ok_or_else(|| ApiError::new(404, format!("change {at:?} not in {doc_id}")))?;
    let from = snapshot_at(dir, doc_id, &change.deps)?;
    let to = snapshot_at(dir, doc_id, std::slice::from_ref(&change.hash))?;
    crate::route::to_value(&diff_projects(&from, &to))
}

/// Cap diff payload text: bodies can be up to the 256 KiB share cap.
fn clip(s: &str) -> String {
    const MAX: usize = 2000;
    if s.len() <= MAX {
        s.to_string()
    } else {
        let mut end = MAX;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

fn diff_projects(from: &SharedProject, to: &SharedProject) -> HistoryDiff {
    let mut d = HistoryDiff::default();

    let from_papers: std::collections::HashMap<&str, &_> = from
        .papers
        .iter()
        .map(|p| (p.source_id.as_str(), p))
        .collect();
    let to_papers: std::collections::HashMap<&str, &_> = to
        .papers
        .iter()
        .map(|p| (p.source_id.as_str(), p))
        .collect();
    for p in &to.papers {
        if !from_papers.contains_key(p.source_id.as_str()) {
            d.papers_added.push(PaperChange {
                source_id: p.source_id.clone(),
                title: p.title.clone(),
            });
        }
    }
    for p in &from.papers {
        if !to_papers.contains_key(p.source_id.as_str()) {
            d.papers_removed.push(PaperChange {
                source_id: p.source_id.clone(),
                title: p.title.clone(),
            });
        }
    }

    d.tags_added = to
        .tags
        .iter()
        .filter(|t| !from.tags.contains(t))
        .cloned()
        .collect();
    d.tags_removed = from
        .tags
        .iter()
        .filter(|t| !to.tags.contains(t))
        .cloned()
        .collect();

    let from_notes: std::collections::HashMap<&str, &_> =
        from.notes.iter().map(|n| (n.uuid.as_str(), n)).collect();
    for n in &to.notes {
        match from_notes.get(n.uuid.as_str()) {
            None => d.notes_added.push(EntryChange {
                uuid: n.uuid.clone(),
                title: n.title.clone(),
                from: None,
                to: Some(clip(&n.body)),
            }),
            Some(o) if o.title != n.title || o.body != n.body => {
                d.notes_changed.push(EntryChange {
                    uuid: n.uuid.clone(),
                    title: n.title.clone(),
                    from: Some(clip(&o.body)),
                    to: Some(clip(&n.body)),
                })
            }
            Some(_) => {}
        }
    }
    let to_notes: std::collections::HashSet<&str> =
        to.notes.iter().map(|n| n.uuid.as_str()).collect();
    for n in &from.notes {
        if !to_notes.contains(n.uuid.as_str()) {
            d.notes_removed.push(EntryChange {
                uuid: n.uuid.clone(),
                title: n.title.clone(),
                from: Some(clip(&n.body)),
                to: None,
            });
        }
    }

    let from_anns: std::collections::HashMap<&str, &_> = from
        .annotations
        .iter()
        .map(|a| (a.uuid.as_str(), a))
        .collect();
    for a in &to.annotations {
        match from_anns.get(a.uuid.as_str()) {
            None => d.annotations_added.push(EntryChange {
                uuid: a.uuid.clone(),
                title: a.paper_source_id.clone(),
                from: None,
                to: Some(clip(&a.comment)),
            }),
            Some(o) if o.comment != a.comment => d.annotations_changed.push(EntryChange {
                uuid: a.uuid.clone(),
                title: a.paper_source_id.clone(),
                from: Some(clip(&o.comment)),
                to: Some(clip(&a.comment)),
            }),
            Some(_) => {}
        }
    }
    let to_anns: std::collections::HashSet<&str> =
        to.annotations.iter().map(|a| a.uuid.as_str()).collect();
    for a in &from.annotations {
        if !to_anns.contains(a.uuid.as_str()) {
            d.annotations_removed.push(EntryChange {
                uuid: a.uuid.clone(),
                title: a.paper_source_id.clone(),
                from: Some(clip(&a.comment)),
                to: None,
            });
        }
    }

    let mut meta = |field: &str, f: &str, t: &str| {
        if f != t {
            d.meta.push(FieldChange {
                field: field.to_string(),
                from: f.to_string(),
                to: t.to_string(),
            });
        }
    };
    meta("name", &from.name, &to.name);
    meta("description", &from.description, &to.description);
    meta(
        "color",
        &from.color.map(|c| c.to_string()).unwrap_or_default(),
        &to.color.map(|c| c.to_string()).unwrap_or_default(),
    );
    d
}

/// `POST /api/history/{scope}/restore` receipt.
#[derive(Serialize, ts_rs::TS)]
pub struct RestoredToChange {
    pub ok: bool,
    pub removed_papers: usize,
    pub removed_notes: usize,
    pub removed_annotations: usize,
    pub removed_tags: usize,
}

impl From<RemovalOutcome> for RestoredToChange {
    fn from(r: RemovalOutcome) -> Self {
        Self {
            ok: true,
            removed_papers: r.papers,
            removed_notes: r.notes,
            removed_annotations: r.annotations,
            removed_tags: r.tags,
        }
    }
}

#[derive(serde::Deserialize, ts_rs::TS)]
pub struct RestoreBody {
    /// Change hash to restore to (state as of that change, inclusive).
    pub to: String,
}

fn restore_project(state: &AppState, fk: i64, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let body: RestoreBody = ctx.parse_body()?;
    let removed = journal::restore_project(state, &journal::journal_dir(), fk, &body.to)?;
    crate::route::to_value(&RestoredToChange::from(removed))
}

fn restore_library(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let body: RestoreBody = ctx.parse_body()?;
    let removed = journal::restore_library(state, &journal::journal_dir(), &body.to)?;
    crate::route::to_value(&RestoredToChange::from(removed))
}

/// Which role dir holds this share's doc file (hoster, reader, e2ee either).
fn share_doc_dir(share_id: &str) -> Result<PathBuf, ApiError> {
    if !valid_share_id(share_id) {
        return Err(ApiError::new(404, format!("share {share_id:?} not found")));
    }
    let share_dir = linxiv_core::config::data_dir().join("share");
    let candidates = [
        share_dir.clone(),
        received_dir(&share_dir),
        e2ee_dir(&share_dir),
        e2ee_received_dir(&share_dir),
    ];
    candidates
        .into_iter()
        .find(|d| linxiv_share::doc_path(d, share_id).is_file())
        .ok_or_else(|| ApiError::new(404, format!("share {share_id:?} not found")))
}
