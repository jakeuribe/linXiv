//! Local change journal: automerge snapshots of every project + the library,
//! written at the top of each sync pass (nudge-driven, same debounce). SQLite
//! stays authoritative; history/undo read the journal. Docs live at
//! `<data_dir>/journal/p<fk>.automerge` + `library.automerge` — ids are keyed
//! by project fk, never SHARE_ID (minting one would mark the project
//! share-linked and e.g. block paper merges).

use std::path::{Path, PathBuf};

use rusqlite::Connection;

use linxiv_core::models::Status;
use linxiv_core::service::{
    author as author_svc, note as note_svc, paper as paper_svc, project as project_svc,
};
use linxiv_share::{
    apply_content, apply_removals, build_project_snapshot, save, snapshot_at, RemovalOutcome,
    ShareError, SharedAnnotation, SharedNote, SharedPaper, SharedProject,
};

use crate::route::ApiError;
use crate::state::AppState;

/// Doc id of the library-wide journal (papers in no project, library notes).
pub const LIBRARY_DOC: &str = "library";

pub fn journal_dir() -> PathBuf {
    linxiv_core::config::data_dir().join("journal")
}

pub fn project_doc_id(project_fk: i64) -> String {
    format!("p{project_fk}")
}

static NUDGE: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Poke the journal loop. `share_sync::nudge` calls this — every successful
/// non-GET on `route()` and on the share front door (`share::dispatch`).
pub fn nudge() {
    NUDGE.notify_one();
}

/// Sleep until the next journal pass is due: a debounced nudge, or the
/// interval (same cadence constants as share sync).
pub async fn next_due() {
    crate::share_sync::next_sync_due_on(
        &NUDGE,
        crate::share_sync::INTERVAL_SYNC_PERIOD,
        crate::share_sync::NUDGE_DEBOUNCE,
    )
    .await;
}

/// Journal loop: one pass now, then on every nudge (debounced) or interval
/// tick. Spawned UNCONDITIONALLY by every front door — history/undo must work
/// with p2p unbound, unlike the share-sync loop, which gates on the node.
/// (The Tauri app spawns its own twin over managed state.)
pub fn spawn_journal_loop(state: std::sync::Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            write_all(&state, &journal_dir());
            next_due().await;
        }
    });
}

/// 404 unless `to` names a change actually in this doc's history — a stale or
/// foreign hash must fail cleanly, never feed the destructive apply.
fn require_change(dir: &Path, doc_id: &str, to: &str) -> Result<(), ApiError> {
    if !linxiv_share::doc_history(dir, doc_id)?
        .iter()
        .any(|c| c.hash == to)
    {
        return Err(ApiError::new(404, format!("change {to:?} not in {doc_id}")));
    }
    Ok(())
}

/// One journal pass: evolve every non-deleted project's doc + the library doc.
/// `save` skips no-op writes, so quiet passes cost reads only. Log-and-continue.
// ponytail: full snapshot rebuild + reconcile per pass, and docs grow forever
// (no compaction); upgrade to dirty-tracking + a compaction policy if large
// libraries make the debounced pass noticeable.
pub fn write_all(state: &AppState, dir: &Path) {
    let projects =
        match state.with_conn(|c| project_svc::get_many(c, &project_svc::Projects::default())) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("journal: project list: {e}");
                return;
            }
        };
    for p in projects.iter().filter(|p| p.status != Status::Deleted) {
        let Some(fk) = p.id else { continue };
        let res = state.with_conn(|c| {
            build_project_snapshot(c, fk, project_doc_id(fk)).and_then(|sp| save(dir, &sp))
        });
        if let Err(e) = res {
            eprintln!("journal: project {fk}: {e}");
        }
    }
    // Purge docs whose project row no longer exists at all: SQLite reuses
    // PROJECT_FK rowids after a hard delete, and a reused fk must not inherit
    // the purged project's history. Trashed projects keep their docs.
    let live: std::collections::HashSet<i64> = projects.iter().filter_map(|p| p.id).collect();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let name = e.file_name();
            let stale = name
                .to_str()
                .and_then(|n| n.strip_prefix('p'))
                .and_then(|n| n.strip_suffix(".automerge"))
                .and_then(|n| n.parse::<i64>().ok())
                .is_some_and(|fk| !live.contains(&fk));
            if stale {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let res = state.with_conn(|c| build_library_snapshot(c).and_then(|sp| save(dir, &sp)));
    if let Err(e) = res {
        eprintln!("journal: library: {e}");
    }
}

/// The library snapshot: every active paper, plus project-less notes/annotations.
fn build_library_snapshot(conn: &Connection) -> Result<SharedProject, ShareError> {
    let details = paper_svc::list_papers(conn, true, None, 0, None)?;
    let paper_ids: Vec<i64> = details.iter().map(|p| p.paper_id).collect();
    let mut orcids = author_svc::paper_author_orcids(conn, &paper_ids)?;
    let mut papers = Vec::with_capacity(details.len());
    let mut fks = Vec::with_capacity(details.len());
    for p in details {
        fks.push(p.source_fk);
        papers.push(SharedPaper {
            source_id: p.source_id,
            version: p.version,
            published: p.published.map(|d| d.to_string()),
            title: p.title,
            summary: p.summary.unwrap_or_default(),
            authors: p.authors,
            author_orcids: orcids.remove(&p.paper_id).unwrap_or_default(),
            tags: p.tags,
            pdf_blob: None,
        });
    }

    let mut lib_notes = note_svc::list_all(conn)?;
    lib_notes.retain(|n| n.project_id.is_none());
    let mut lib_anns = linxiv_core::service::annotation::list_all(conn)?;
    lib_anns.retain(|a| a.project_id.is_none());
    let note_fks: Vec<i64> = lib_notes
        .iter()
        .map(|n| n.source_fk)
        .chain(lib_anns.iter().map(|a| a.source_fk))
        .chain(fks)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let source_ids = paper_svc::source_ids_by_fk(conn, &note_fks)?;

    let notes = lib_notes
        .into_iter()
        .filter_map(|n| {
            Some(SharedNote {
                paper_source_id: Some(source_ids.get(&n.source_fk)?.clone()),
                uuid: n.uuid,
                title: n.title,
                body: n.content,
                created_at: n.created_at.map(|t| t.to_string()),
                updated_at: n.updated_at.map(|t| t.to_string()),
            })
        })
        .collect();
    let annotations = lib_anns
        .into_iter()
        .filter_map(|a| {
            Some(SharedAnnotation {
                paper_source_id: source_ids.get(&a.source_fk)?.clone(),
                uuid: a.uuid,
                anchor: a.anchor,
                comment: a.comment,
                created_at: a.created_at.map(|t| t.to_string()),
                updated_at: a.updated_at.map(|t| t.to_string()),
            })
        })
        .collect();

    Ok(SharedProject {
        share_id: LIBRARY_DOC.to_string(),
        name: "Library".to_string(),
        description: String::new(),
        color: None,
        tags: Vec::new(),
        papers,
        notes,
        annotations,
    })
}

/// Restore a project to its state as of change `to` — destructive in both
/// directions: snapshot content comes back (additive apply), later additions
/// are removed (removal apply), and the project row is replaced wholesale.
pub fn restore_project(
    state: &AppState,
    dir: &Path,
    project_fk: i64,
    to: &str,
) -> Result<RemovalOutcome, ApiError> {
    let doc_id = project_doc_id(project_fk);
    require_change(dir, &doc_id, to)?;
    let old = snapshot_at(dir, &doc_id, &[to.to_string()])?;
    state.with_conn(|c| -> Result<RemovalOutcome, ApiError> {
        let cur = build_project_snapshot(c, project_fk, doc_id.clone())?;
        project_svc::update(
            c,
            &linxiv_core::models::ProjectUpdateIn {
                project_fk,
                name: Some(old.name.clone()),
                description: Some(old.description.clone()),
                color: Some(old.color.and_then(|v| i32::try_from(v).ok())),
                project_tags: Some(old.tags.clone()),
                status: None,
            },
        )?;
        apply_content(c, &old, Some(project_fk), false, true)?;
        Ok(apply_removals(c, &cur, &old, Some(project_fk))?)
    })
}

/// Restore the library to its state as of change `to`. Papers deleted since
/// come back from the trash; papers added since are trashed (never purged).
pub fn restore_library(state: &AppState, dir: &Path, to: &str) -> Result<RemovalOutcome, ApiError> {
    require_change(dir, LIBRARY_DOC, to)?;
    let old = snapshot_at(dir, LIBRARY_DOC, &[to.to_string()])?;
    state.with_conn(|c| -> Result<RemovalOutcome, ApiError> {
        let cur = build_library_snapshot(c)?;
        apply_content(c, &old, None, true, true)?;
        Ok(apply_removals(c, &cur, &old, None)?)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use linxiv_core::models::{NoteIn, PaperIn, ProjectIn};
    use linxiv_core::storage;
    use linxiv_share::doc_history;
    use tempfile::TempDir;

    /// Seeded state: two papers, a project holding both, one project note.
    fn seeded() -> (AppState, i64) {
        let mut conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        let pin = |sid: &str, title: &str| PaperIn {
            title: title.into(),
            published: NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            source_id: Some(sid.into()),
            version: None,
            authors: Some(vec!["A".into()]),
            summary: Some("s".into()),
            category: Some("cs.LG".into()),
            doi: None,
            url: None,
            tags: None,
            source: Some("arxiv".into()),
        };
        paper_svc::upsert(&mut conn, &pin("arxiv:1", "First"), None).unwrap();
        paper_svc::upsert(&mut conn, &pin("arxiv:2", "Second"), None).unwrap();
        let fk1 = paper_svc::ensure_paper_root(&mut conn, "arxiv:1").unwrap();
        let fk2 = paper_svc::ensure_paper_root(&mut conn, "arxiv:2").unwrap();
        let pid = project_svc::create(
            &mut conn,
            &ProjectIn {
                name: "P".into(),
                description: String::new(),
                color: None,
                tags: vec!["keep".into()],
                source_fks: vec![fk1, fk2],
            },
        )
        .unwrap();
        note_svc::create(
            &conn,
            &NoteIn {
                source_fk: fk1,
                title: "seed note".into(),
                content: "body".into(),
                paper_id: None,
                project_fk: Some(pid),
                uuid: None,
            },
        )
        .unwrap();
        let state = AppState::from_parts(conn, std::env::temp_dir(), std::env::temp_dir());
        (state, pid)
    }

    #[test]
    fn restore_project_undoes_removal_and_later_addition() {
        let (state, pid) = seeded();
        let dir = TempDir::new().unwrap();

        write_all(&state, dir.path());
        let doc_id = project_doc_id(pid);
        let log = doc_history(dir.path(), &doc_id).unwrap();
        assert_eq!(log.len(), 1);

        // Mutate: unlink a paper, add a note, edit the seed note's body (its
        // UPDATED_AT is now newer than the snapshot — restore must still win).
        state.with_conn(|c| {
            project_svc::remove_papers(c, pid, &["arxiv:1".to_string()]).unwrap();
            let fk2 = paper_svc::ensure_paper_root(c, "arxiv:2").unwrap();
            note_svc::create(
                c,
                &NoteIn {
                    source_fk: fk2,
                    title: "later note".into(),
                    content: "x".into(),
                    paper_id: None,
                    project_fk: Some(pid),
                    uuid: None,
                },
            )
            .unwrap();
            let seed_note = note_svc::get_many(
                c,
                &note_svc::Notes {
                    project_fk: Some(pid),
                    ..Default::default()
                },
            )
            .unwrap()
            .into_iter()
            .find(|n| n.title == "seed note")
            .unwrap();
            note_svc::update(
                c,
                &linxiv_core::models::NoteUpdateIn {
                    note_id: seed_note.note_id,
                    title: None,
                    content: Some("edited later".into()),
                },
            )
            .unwrap();
        });
        write_all(&state, dir.path());
        assert_eq!(doc_history(dir.path(), &doc_id).unwrap().len(), 2);

        // A hash not in this doc's history must 404, never feed the
        // destructive apply.
        let bogus = "0".repeat(64);
        assert_eq!(
            restore_project(&state, dir.path(), pid, &bogus)
                .unwrap_err()
                .status,
            404
        );

        let removed = restore_project(&state, dir.path(), pid, &log[0].hash).unwrap();
        assert_eq!(removed.notes, 1, "the later note is removed");

        let p = state
            .with_conn(|c| project_svc::get_required(c, pid))
            .unwrap();
        let fk1 = state
            .with_conn(|c| paper_svc::ensure_paper_root(c, "arxiv:1"))
            .unwrap();
        assert!(p.source_fks.contains(&fk1), "unlinked paper is re-linked");
        let notes = state
            .with_conn(|c| {
                note_svc::get_many(
                    c,
                    &note_svc::Notes {
                        project_fk: Some(pid),
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "seed note");
        assert_eq!(
            notes[0].content, "body",
            "a later edit must be reverted even though its UPDATED_AT is newer"
        );
    }

    // SQLite reuses PROJECT_FK rowids after a hard delete; write_all must purge
    // docs whose project row is gone so a reused fk can't inherit history.
    #[test]
    fn write_all_purges_docs_of_hard_deleted_projects() {
        let (state, pid) = seeded();
        let dir = TempDir::new().unwrap();
        write_all(&state, dir.path());
        let live_doc = dir.path().join(format!("p{pid}.automerge"));
        assert!(live_doc.is_file());
        // A doc for a project fk that doesn't exist must be swept.
        let stale = dir.path().join("p999.automerge");
        std::fs::write(&stale, b"stale").unwrap();
        write_all(&state, dir.path());
        assert!(!stale.exists());
        assert!(live_doc.is_file());
    }

    #[test]
    fn restore_library_untrashes_deleted_and_trashes_added() {
        let (state, _pid) = seeded();
        let dir = TempDir::new().unwrap();

        write_all(&state, dir.path());
        let log = doc_history(dir.path(), LIBRARY_DOC).unwrap();
        assert_eq!(log.len(), 1);

        // Trash one paper, add a new one.
        state.with_conn(|c| {
            paper_svc::delete(c, &paper_svc::PaperRef::source("arxiv:1".to_string())).unwrap();
            paper_svc::upsert(
                c,
                &PaperIn {
                    title: "Third".into(),
                    published: NaiveDate::from_ymd_opt(2024, 2, 1).unwrap(),
                    source_id: Some("arxiv:3".into()),
                    version: None,
                    authors: None,
                    summary: None,
                    category: None,
                    doi: None,
                    url: None,
                    tags: None,
                    source: Some("arxiv".into()),
                },
                None,
            )
            .unwrap();
        });
        write_all(&state, dir.path());

        restore_library(&state, dir.path(), &log[0].hash).unwrap();
        let deleted = |c: &mut rusqlite::Connection, sid: &str| {
            storage::queries::paper::is_paper_deleted(c, sid).unwrap()
        };
        assert!(!state.with_conn(|c| deleted(c, "arxiv:1")), "untrashed");
        assert!(
            state.with_conn(|c| deleted(c, "arxiv:3")),
            "later addition trashed"
        );
    }
}
