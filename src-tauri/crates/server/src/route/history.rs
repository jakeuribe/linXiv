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
        // This device's journal actor. The UI fetches it LOCALLY (apiFetch,
        // never the remote backend) so "mine" reflects the viewer's device,
        // not whichever node served the timeline.
        ("GET", ["api", "history", "actor"]) => Some(crate::route::to_value(&DeviceActor {
            actor: linxiv_share::device_actor_hex(),
        })),
        ("GET", ["api", "history", "library"]) => {
            Some(timeline(&journal::journal_dir(), journal::LIBRARY_DOC))
        }
        ("GET", ["api", "history", "library", "diff"]) => Some(change_diff(
            state,
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
            change_diff(
                state,
                &journal::journal_dir(),
                &journal::project_doc_id(fk),
                ctx,
            )
        })),
        ("POST", ["api", "history", "project", fk, "restore"]) => {
            Some(path_i64(fk).and_then(|fk| restore_project(state, fk, ctx)))
        }
        ("GET", ["api", "history", "share", id]) => {
            Some(share_doc_dir(id).and_then(|d| timeline(&d, id)))
        }
        ("GET", ["api", "history", "share", id, "diff"]) => {
            Some(share_doc_dir(id).and_then(|d| change_diff(state, &d, id, ctx)))
        }
        _ => None,
    }
}

/// `GET /api/history/actor`: this device's journal actor (None before init).
#[derive(Serialize, ts_rs::TS)]
pub struct DeviceActor {
    pub actor: Option<String>,
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
    /// Host-assigned name from the node's Member List; None when unnamed.
    pub display_name: Option<String>,
}

/// `GET /api/history/{scope}` envelope: oldest-first change log + our actor.
#[derive(Serialize, ts_rs::TS)]
pub struct Timeline {
    pub changes: Vec<ChangeRow>,
    pub device_actor: Option<String>,
}

/// actor hex -> Member List name. A member's endpoint id resolves too:
/// remote-query writes journal under it.
fn name_map(
    members: Vec<crate::remote_query::Member>,
) -> std::collections::HashMap<String, String> {
    let mut names = std::collections::HashMap::new();
    for m in members {
        if let Some(name) = m.name.filter(|n| !n.is_empty()) {
            names.insert(m.id.to_ascii_lowercase(), name.clone());
            for a in m.actors {
                names.insert(a.to_ascii_lowercase(), name.clone());
            }
        }
    }
    names
}

fn timeline(dir: &std::path::Path, doc_id: &str) -> Result<Value, ApiError> {
    // Missing/empty member file (the desktop case) yields an empty map —
    // every row's display_name stays None.
    let names = name_map(crate::remote_query::load_members().unwrap_or_default());
    timeline_with(dir, doc_id, &names)
}

fn timeline_with(
    dir: &std::path::Path,
    doc_id: &str,
    names: &std::collections::HashMap<String, String>,
) -> Result<Value, ApiError> {
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
            display_name: names.get(&c.actor.to_ascii_lowercase()).cloned(),
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
    /// Local `/library/{sfk}` target; None when unresolvable or trashed.
    pub source_fk: Option<i64>,
}

/// Added/removed carry no body; `changed` rows carry truncated from→to text.
#[derive(Serialize, ts_rs::TS)]
pub struct EntryChange {
    pub uuid: String,
    pub title: String,
    pub from: Option<String>,
    pub to: Option<String>,
    /// Notes only: local `/notes/{id}` target; None when unresolvable.
    pub note_id: Option<i64>,
    /// Annotations only: their paper's `/library/{sfk}`; None when unresolvable.
    pub paper_sfk: Option<i64>,
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

fn change_diff(
    state: &AppState,
    dir: &std::path::Path,
    doc_id: &str,
    ctx: &ReqCtx<'_>,
) -> Result<Value, ApiError> {
    let at = ctx
        .q("at")
        .ok_or_else(|| ApiError::new(422, "missing ?at=<change hash>"))?;
    crate::route::to_value(&diff_at(state, dir, doc_id, &at)?)
}

fn diff_at(
    state: &AppState,
    dir: &std::path::Path,
    doc_id: &str,
    at: &str,
) -> Result<HistoryDiff, ApiError> {
    let history = doc_history(dir, doc_id)?;
    let change = history
        .iter()
        .find(|c| c.hash == at)
        .ok_or_else(|| ApiError::new(404, format!("change {at:?} not in {doc_id}")))?;
    let from = snapshot_at(dir, doc_id, &change.deps)?;
    let to = snapshot_at(dir, doc_id, std::slice::from_ref(&change.hash))?;
    let mut d = diff_projects(&from, &to);
    state.with_conn(|conn| link_targets(conn, &mut d));
    annotation_titles(&from, &to, &mut d);
    Ok(d)
}

/// Best-effort local link targets on a fresh diff: rows that don't resolve
/// against this DB (share diffs of foreign content, deleted items) stay None.
fn link_targets(conn: &rusqlite::Connection, d: &mut HistoryDiff) {
    // One batched lookup for every paper/annotation row; trashed roots are
    // absent from the map, so they never link.
    let mut sids: Vec<String> = d
        .papers_added
        .iter()
        .chain(d.papers_removed.iter())
        .map(|p| p.source_id.clone())
        .chain(
            d.annotations_added
                .iter()
                .chain(d.annotations_changed.iter())
                .chain(d.annotations_removed.iter())
                .map(|a| a.title.clone()),
        )
        .collect();
    sids.sort();
    sids.dedup();
    let fks = linxiv_core::service::paper::active_source_fks(conn, &sids).unwrap_or_default();
    for p in d.papers_added.iter_mut().chain(d.papers_removed.iter_mut()) {
        p.source_fk = fks.get(&p.source_id).copied();
    }
    let uuids: Vec<String> = d
        .notes_added
        .iter()
        .chain(d.notes_changed.iter())
        .chain(d.notes_removed.iter())
        .map(|n| n.uuid.clone())
        .collect();
    let note_ids = linxiv_core::service::note::ids_by_uuid(conn, &uuids).unwrap_or_default();
    for n in d
        .notes_added
        .iter_mut()
        .chain(d.notes_changed.iter_mut())
        .chain(d.notes_removed.iter_mut())
    {
        n.note_id = note_ids.get(&n.uuid).copied();
    }
    // An annotation EntryChange's title is still its paper_source_id here
    // (diff_projects); annotation_titles swaps in the paper title afterwards.
    for a in d
        .annotations_added
        .iter_mut()
        .chain(d.annotations_changed.iter_mut())
        .chain(d.annotations_removed.iter_mut())
    {
        a.paper_sfk = fks.get(&a.title).copied();
    }
}

/// Annotations have no title of their own: after link resolution, swap the
/// paper_source_id in `title` for the paper's title from the snapshots
/// (prefer `to`). Papers absent from both snapshots keep the raw source id.
fn annotation_titles(from: &SharedProject, to: &SharedProject, d: &mut HistoryDiff) {
    let titles: std::collections::HashMap<&str, &str> = from
        .papers
        .iter()
        .chain(to.papers.iter())
        .map(|p| (p.source_id.as_str(), p.title.as_str()))
        .collect();
    for a in d
        .annotations_added
        .iter_mut()
        .chain(d.annotations_changed.iter_mut())
        .chain(d.annotations_removed.iter_mut())
    {
        if let Some(t) = titles.get(a.title.as_str()).filter(|t| !t.is_empty()) {
            a.title = t.to_string();
        }
    }
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
                source_fk: None,
            });
        }
    }
    for p in &from.papers {
        if !to_papers.contains_key(p.source_id.as_str()) {
            d.papers_removed.push(PaperChange {
                source_id: p.source_id.clone(),
                title: p.title.clone(),
                source_fk: None,
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
                note_id: None,
                paper_sfk: None,
            }),
            Some(o) if o.title != n.title || o.body != n.body => {
                d.notes_changed.push(EntryChange {
                    uuid: n.uuid.clone(),
                    title: n.title.clone(),
                    from: Some(clip(&o.body)),
                    to: Some(clip(&n.body)),
                    note_id: None,
                    paper_sfk: None,
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
                note_id: None,
                paper_sfk: None,
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
                note_id: None,
                paper_sfk: None,
            }),
            Some(o) if o.comment != a.comment => d.annotations_changed.push(EntryChange {
                uuid: a.uuid.clone(),
                title: a.paper_source_id.clone(),
                from: Some(clip(&o.comment)),
                to: Some(clip(&a.comment)),
                note_id: None,
                paper_sfk: None,
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
                note_id: None,
                paper_sfk: None,
            });
        }
    }

    // clip like every entry branch: project descriptions are unbounded.
    let mut meta = |field: &str, f: &str, t: &str| {
        if f != t {
            d.meta.push(FieldChange {
                field: field.to_string(),
                from: clip(f),
                to: clip(t),
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

#[cfg(test)]
mod tests {
    use super::{diff_at, name_map, timeline_with};
    use crate::journal;
    use crate::remote_query::{Member, Role};
    use crate::state::AppState;
    use chrono::NaiveDate;
    use linxiv_core::models::{AnnotationIn, NoteIn, PaperIn};
    use linxiv_core::service::{annotation as ann_svc, note as note_svc, paper as paper_svc};
    use linxiv_core::storage;
    use tempfile::TempDir;

    /// Timeline rows resolve display_name via the member name map; diff rows
    /// carry local link targets (paper source_fk, note note_id). Exercised
    /// through the env-free cores — tests must not redirect the data dir.
    #[tokio::test]
    async fn timeline_names_and_diff_link_targets() {
        let tmpdir = TempDir::new().unwrap();
        let dir = tmpdir.path();

        let mut conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        paper_svc::upsert(
            &mut conn,
            &PaperIn {
                title: "First".into(),
                published: NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
                source_id: Some("arxiv:1".into()),
                version: None,
                authors: Some(vec!["A".into()]),
                summary: Some("s".into()),
                category: None,
                doi: None,
                url: None,
                tags: None,
                source: Some("arxiv".into()),
            },
            None,
        )
        .unwrap();
        let fk = paper_svc::ensure_paper_root(&mut conn, "arxiv:1").unwrap();
        // A library note (no project) so the library doc journals it.
        note_svc::create(
            &conn,
            &NoteIn {
                source_fk: fk,
                title: "lib note".into(),
                content: "body".into(),
                paper_id: None,
                project_fk: None,
                uuid: None,
            },
        )
        .unwrap();
        // A library annotation (no project) on that paper.
        ann_svc::create(
            &conn,
            &AnnotationIn {
                source_fk: fk,
                anchor: r##"{"v":1,"version":1,"page":1,"color":"#ffd400","quote":"q","rects":[{"x":0,"y":0,"w":0.5,"h":0.1}]}"##.into(),
                comment: "hi".into(),
                project_fk: None,
                uuid: None,
            },
        )
        .unwrap();
        let state = AppState::from_parts(conn, std::env::temp_dir(), std::env::temp_dir());
        journal::write_all(&state, dir);

        // Empty name map: every display_name is None.
        let empty = name_map(Vec::new());
        let tl = timeline_with(dir, journal::LIBRARY_DOC, &empty).unwrap();
        let row = &tl["changes"][0];
        assert!(row["display_name"].is_null());
        let actor = row["actor"].as_str().unwrap().to_string();
        let hash = row["hash"].as_str().unwrap().to_string();

        // Name that actor on a member; lookup is case-insensitive, and the
        // member's endpoint id resolves as an actor too.
        let endpoint = "aa".repeat(32);
        let names = name_map(vec![Member {
            id: endpoint.to_uppercase(),
            role: Role::Read,
            name: Some("Ada".into()),
            actors: vec![actor.to_uppercase()],
        }]);
        assert_eq!(names.get(&actor).map(String::as_str), Some("Ada"));
        assert_eq!(names.get(&endpoint).map(String::as_str), Some("Ada"));
        let tl = timeline_with(dir, journal::LIBRARY_DOC, &names).unwrap();
        assert_eq!(tl["changes"][0]["display_name"], "Ada");

        // The first change adds the paper and the note; both rows link.
        let diff = diff_at(&state, dir, journal::LIBRARY_DOC, &hash).unwrap();
        assert_eq!(diff.papers_added[0].source_fk, Some(fk));
        let note_id = state
            .with_conn(|c| note_svc::list_all(c))
            .unwrap()
            .pop()
            .unwrap()
            .note_id;
        assert_eq!(diff.notes_added[0].note_id, Some(note_id));
        // The annotation row carries its paper's title, not the raw source
        // id, and still links to the paper.
        assert_eq!(diff.annotations_added[0].title, "First");
        assert_eq!(diff.annotations_added[0].paper_sfk, Some(fk));
    }
}
