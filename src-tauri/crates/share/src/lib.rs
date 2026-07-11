//! linxiv-share — Phase-0 quarantined CRDT store for "shared projects".
//!
//! A shared project is a read-only snapshot of one canonical project's subgraph
//! (project row + its papers/authors/tags + project notes + project tags), held
//! as an automerge document so a later phase can sync it peer-to-peer. Quarantine
//! guarantee: publishing only READS `papers.db` through the core service read
//! APIs; it never writes canonical tables. The documents are the whole store —
//! one `<share_id>.automerge` file per shared project under a share directory.
//!
//! The share directory is injected (`ShareStore::new(path)`); production resolves
//! it as `config::data_dir()/share`. Phase 1 adds a one-way iroh transport (see
//! `transport`) that serves these docs over a capability-gated ALPN.

mod model;
mod transport;

use std::path::{Path, PathBuf};

use automerge::AutoCommit;
use rusqlite::Connection;

use linxiv_core::error::CoreError;
use linxiv_core::service::{
    annotation as annotation_svc, note as note_svc, paper as paper_svc, project as project_svc,
};

pub use model::{SharedAnnotation, SharedNote, SharedPaper, SharedProject, SharedSummary};
pub use transport::{mint_capability, resolve_capability, CapToken, ShareNode, ShareTicket, ALPN};

const SHARE_EXT: &str = "automerge";

#[derive(Debug, thiserror::Error)]
pub enum ShareError {
    #[error("shared project not found: {0}")]
    NotFound(String),
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error("share io error: {0}")]
    Io(#[from] std::io::Error),
    /// network / iroh transport failures, flattened to a message.
    #[error("transport error: {0}")]
    Transport(String),
    /// automerge / autosurgeon (de)serialization failures, flattened to a message.
    #[error("crdt error: {0}")]
    Crdt(String),
}

pub type Result<T> = std::result::Result<T, ShareError>;

// ── canonical → CRDT projection (READ-ONLY) ─────────────────────────────────

/// Gather a canonical project's subgraph read-only and project it onto the CRDT
/// model. Missing project → `ShareError::NotFound`. The only reads are core
/// service `get`/`get_many` calls — no raw SQL, no canonical writes.
pub fn build_shared_project(conn: &Connection, project_id: i64) -> Result<SharedProject> {
    let project = project_svc::get(
        conn,
        &project_svc::Project {
            project_fk: Some(project_id),
        },
    )?
    .ok_or_else(|| ShareError::NotFound(project_id.to_string()))?;

    // Empty source_fks must short-circuit: paper::get_many treats an empty filter
    // list as "no filter" and would return every paper in the library.
    let papers = if project.source_fks.is_empty() {
        Vec::new()
    } else {
        paper_svc::get_many(
            conn,
            &paper_svc::Papers {
                source_fks: Some(project.source_fks.clone()),
                ..Default::default()
            },
        )?
        .into_iter()
        .map(|p| SharedPaper {
            source_id: p.source_id,
            version: p.version,
            title: p.title,
            summary: p.summary.unwrap_or_default(),
            authors: p.authors,
            tags: p.tags,
        })
        .collect()
    };

    let notes = note_svc::get_many(
        conn,
        &note_svc::Notes {
            project_fk: Some(project_id),
            ..Default::default()
        },
    )?
    .into_iter()
    .map(|n| {
        Ok(SharedNote {
            id: n
                .note_id
                .ok_or_else(|| ShareError::Crdt("note missing id".into()))?,
            title: n.title,
            body: n.content,
            created_at: n.created_at.map(|t| t.to_string()),
            updated_at: n.updated_at.map(|t| t.to_string()),
        })
    })
    .collect::<Result<Vec<_>>>()?;

    let mut annotations = Vec::new();
    for a in annotation_svc::get_many(
        conn,
        &annotation_svc::Annotations {
            project_fk: Some(project_id),
            ..Default::default()
        },
    )? {
        // Skip annotations whose source_id no longer resolves (mirrors build_manifest).
        let Some(paper_source_id) = paper_svc::get_source_id(conn, a.source_fk)? else {
            continue;
        };
        annotations.push(SharedAnnotation {
            id: a.annotation_id,
            paper_source_id,
            anchor: a.anchor,
            comment: a.comment,
            created_at: a.created_at.map(|t| t.to_string()),
            updated_at: a.updated_at.map(|t| t.to_string()),
        });
    }

    Ok(SharedProject {
        share_id: project_id.to_string(),
        name: project.name,
        description: project.description,
        color: project.color.map(i64::from),
        tags: project.project_tags,
        papers,
        notes,
        annotations,
    })
}

// ── persistence (the store) ─────────────────────────────────────────────────

fn doc_path(share_dir: &Path, share_id: &str) -> PathBuf {
    share_dir.join(format!("{share_id}.{SHARE_EXT}"))
}

fn crdt<E: std::fmt::Display>(e: E) -> ShareError {
    ShareError::Crdt(e.to_string())
}

/// Reconcile `sp` into a fresh automerge doc and write `<share_id>.automerge`.
/// Creates `share_dir` if absent; overwrites idempotently.
pub fn save(share_dir: &Path, sp: &SharedProject) -> Result<()> {
    std::fs::create_dir_all(share_dir)?;
    let mut doc = AutoCommit::new();
    autosurgeon::reconcile(&mut doc, sp).map_err(crdt)?;
    // Write to a sibling temp file then rename, so a crash mid-write can't leave a
    // truncated <share_id>.automerge that later fails to load.
    let final_path = doc_path(share_dir, &sp.share_id);
    let tmp_path = share_dir.join(format!("{}.{SHARE_EXT}.tmp", sp.share_id));
    std::fs::write(&tmp_path, doc.save())?;
    std::fs::rename(&tmp_path, &final_path)?;
    Ok(())
}

/// Load `<share_id>.automerge` and hydrate it back into a `SharedProject`.
/// Missing file → `ShareError::NotFound`.
pub fn load(share_dir: &Path, share_id: &str) -> Result<SharedProject> {
    let bytes = match std::fs::read(doc_path(share_dir, share_id)) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ShareError::NotFound(share_id.to_string()))
        }
        Err(e) => return Err(e.into()),
    };
    let doc = AutoCommit::load(&bytes).map_err(crdt)?;
    autosurgeon::hydrate(&doc).map_err(crdt)
}

/// Build + save a project's snapshot. Returns the share_id (the project id as a
/// string). Re-publishing the same project overwrites its doc file.
pub fn publish(conn: &Connection, share_dir: &Path, project_id: i64) -> Result<String> {
    let sp = build_shared_project(conn, project_id)?;
    save(share_dir, &sp)?;
    Ok(sp.share_id)
}

/// Summaries of every `*.automerge` doc in `share_dir`. A missing dir is an empty
/// list, not an error.
pub fn list_shared(share_dir: &Path) -> Result<Vec<SharedSummary>> {
    let entries = match std::fs::read_dir(share_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some(SHARE_EXT) {
            continue;
        }
        let Some(share_id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        // Skip a corrupt or partially-written doc rather than failing the whole
        // listing on one bad file.
        let Ok(sp) = load(share_dir, share_id) else {
            continue;
        };
        out.push(SharedSummary {
            share_id: sp.share_id,
            name: sp.name,
            paper_count: sp.papers.len(),
            note_count: sp.notes.len(),
            annotation_count: sp.annotations.len(),
            tag_count: sp.tags.len(),
        });
    }
    Ok(out)
}

/// Hydrate one shared project by id (alias of `load` for the public read API).
pub fn get_shared(share_dir: &Path, share_id: &str) -> Result<SharedProject> {
    load(share_dir, share_id)
}

/// Owns the injected share directory — the test seam mirroring core's
/// `AppState::from_parts` (construct from an explicit path, never from config).
pub struct ShareStore {
    share_dir: PathBuf,
}

impl ShareStore {
    pub fn new(share_dir: impl Into<PathBuf>) -> Self {
        Self {
            share_dir: share_dir.into(),
        }
    }

    pub fn share_dir(&self) -> &Path {
        &self.share_dir
    }

    pub fn publish(&self, conn: &Connection, project_id: i64) -> Result<String> {
        publish(conn, &self.share_dir, project_id)
    }

    pub fn list_shared(&self) -> Result<Vec<SharedSummary>> {
        list_shared(&self.share_dir)
    }

    pub fn get_shared(&self, share_id: &str) -> Result<SharedProject> {
        get_shared(&self.share_dir, share_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use linxiv_core::models::{AnnotationIn, NoteIn, PaperIn, ProjectIn};
    use linxiv_core::storage::{self, db::open_in_memory};

    // annotation_svc comes from `use super::*` (the crate-root service aliases).
    const ANCHOR: &str = r##"{"v":1,"version":1,"page":1,"color":"#ffd400","quote":"q","rects":[{"x":0,"y":0,"w":0.5,"h":0.1}]}"##;

    // Seed a canonical in-memory DB via the real service WRITE APIs and return
    // (conn, project_id). The project has two papers, two project tags, two
    // project notes (plus one library note that must NOT be snapshotted).
    fn seed() -> (Connection, i64) {
        let mut conn = open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();

        let pin = |sid: &str, title: &str, authors: &[&str], tags: &[&str]| PaperIn {
            title: title.into(),
            published: NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            source_id: Some(sid.into()),
            version: None,
            authors: Some(authors.iter().map(|s| s.to_string()).collect()),
            summary: Some(format!("summary of {title}")),
            category: Some("cs.LG".into()),
            doi: None,
            url: None,
            tags: Some(tags.iter().map(|s| s.to_string()).collect()),
            source: Some("arxiv".into()),
        };
        paper_svc::upsert(
            &mut conn,
            &pin("arxiv:1", "First", &["Alice", "Bob"], &["ml"]),
            None,
        )
        .unwrap();
        paper_svc::upsert(
            &mut conn,
            &pin("arxiv:2", "Second", &["Carol"], &["vision"]),
            None,
        )
        .unwrap();
        let fk1 = paper_svc::ensure_paper_root(&mut conn, "arxiv:1").unwrap();
        let fk2 = paper_svc::ensure_paper_root(&mut conn, "arxiv:2").unwrap();

        let project_id = project_svc::create(
            &mut conn,
            &ProjectIn {
                name: "My Project".into(),
                description: "a project".into(),
                color: Some(0x00ff00),
                tags: vec!["RL".into(), "Robotics".into()],
                source_fks: vec![fk1, fk2],
            },
        )
        .unwrap();

        note_svc::create(
            &conn,
            &NoteIn {
                source_fk: fk1,
                title: "note A".into(),
                content: "body A".into(),
                paper_id: None,
                project_fk: Some(project_id),
            },
        )
        .unwrap();
        note_svc::create(
            &conn,
            &NoteIn {
                source_fk: fk2,
                title: "note B".into(),
                content: "body B".into(),
                paper_id: None,
                project_fk: Some(project_id),
            },
        )
        .unwrap();
        // Library note (no project) — must be excluded from the snapshot.
        note_svc::create(
            &conn,
            &NoteIn {
                source_fk: fk1,
                title: "lib".into(),
                content: "library".into(),
                paper_id: None,
                project_fk: None,
            },
        )
        .unwrap();
        // One project-scoped annotation that the snapshot must carry.
        annotation_svc::create(
            &conn,
            &AnnotationIn {
                source_fk: fk1,
                anchor: ANCHOR.into(),
                comment: "highlight".into(),
                project_fk: Some(project_id),
            },
        )
        .unwrap();

        (conn, project_id)
    }

    fn db_checksum(conn: &Connection) -> Vec<(String, i64)> {
        [
            "PAPER", "PAPER_ROOTS", "PROJECT", "PROJECT_TO_PAPER", "NOTE", "ANNOTATION", "TAG",
        ]
        .iter()
        .map(|t| {
            let sql = format!("SELECT COUNT(*) FROM {t}");
            (t.to_string(), conn.query_row(&sql, [], |r| r.get(0)).unwrap())
        })
        .collect()
    }

    #[test]
    fn publish_list_get_roundtrip_matches_seed() {
        let (conn, pid) = seed();
        let dir = tempfile::tempdir().unwrap();
        let store = ShareStore::new(dir.path());

        let share_id = store.publish(&conn, pid).unwrap();
        assert_eq!(share_id, pid.to_string());

        let summaries = store.list_shared().unwrap();
        assert_eq!(summaries.len(), 1);
        let s = &summaries[0];
        assert_eq!(s.share_id, share_id);
        assert_eq!(s.name, "My Project");
        assert_eq!(s.paper_count, 2);
        assert_eq!(s.note_count, 2); // the library note is excluded
        assert_eq!(s.tag_count, 2);
        assert_eq!(s.annotation_count, 1);

        let sp = store.get_shared(&share_id).unwrap();
        assert_eq!(sp.name, "My Project");
        assert_eq!(sp.description, "a project");
        assert_eq!(sp.color, Some(0x00ff00));
        assert_eq!(sp.tags, vec!["RL", "Robotics"]);

        let mut titles: Vec<_> = sp.papers.iter().map(|p| p.title.clone()).collect();
        titles.sort();
        assert_eq!(titles, vec!["First", "Second"]);
        let first = sp.papers.iter().find(|p| p.title == "First").unwrap();
        assert_eq!(first.authors, vec!["Alice", "Bob"]);
        assert_eq!(first.tags, vec!["ml"]);
        assert_eq!(first.summary, "summary of First");

        let mut note_bodies: Vec<_> = sp.notes.iter().map(|n| n.body.clone()).collect();
        note_bodies.sort();
        assert_eq!(note_bodies, vec!["body A", "body B"]);

        // The project-scoped annotation projects into the snapshot.
        assert_eq!(sp.annotations.len(), 1);
        assert_eq!(sp.annotations[0].comment, "highlight");
        assert_eq!(sp.annotations[0].anchor, ANCHOR);
    }

    #[test]
    fn save_load_byte_roundtrip() {
        let (conn, pid) = seed();
        let dir = tempfile::tempdir().unwrap();
        let built = build_shared_project(&conn, pid).unwrap();

        save(dir.path(), &built).unwrap();
        let loaded = load(dir.path(), &built.share_id).unwrap();
        assert_eq!(built, loaded);
    }

    #[test]
    fn missing_project_is_not_found() {
        let (conn, _pid) = seed();
        match build_shared_project(&conn, 9999) {
            Err(ShareError::NotFound(id)) => assert_eq!(id, "9999"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn missing_dir_lists_empty() {
        let dir = tempfile::tempdir().unwrap();
        let absent = dir.path().join("does-not-exist");
        assert!(list_shared(&absent).unwrap().is_empty());
    }

    #[test]
    fn corrupt_doc_is_skipped_not_fatal() {
        let (conn, project_id) = seed();
        let dir = tempfile::tempdir().unwrap();
        publish(&conn, dir.path(), project_id).unwrap();
        // A garbage doc beside the valid one must not break the whole listing.
        std::fs::write(doc_path(dir.path(), "999"), b"not a valid automerge doc").unwrap();
        let listed = list_shared(dir.path()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].share_id, project_id.to_string());
    }

    // The property that justifies a CRDT: two concurrent edits on independent
    // clones converge to the SAME state regardless of merge direction.
    #[test]
    fn crdt_merge_is_convergent_and_order_independent() {
        let (conn, pid) = seed();
        let base = build_shared_project(&conn, pid).unwrap();

        let mut doc0 = AutoCommit::new();
        autosurgeon::reconcile(&mut doc0, &base).unwrap();

        // Two clones share history; each adds a DIFFERENT tag concurrently.
        let mut left = doc0.fork();
        let mut right = doc0.fork();

        let mut lsp: SharedProject = autosurgeon::hydrate(&left).unwrap();
        lsp.tags.push("from-left".into());
        autosurgeon::reconcile(&mut left, &lsp).unwrap();

        let mut rsp: SharedProject = autosurgeon::hydrate(&right).unwrap();
        rsp.tags.push("from-right".into());
        autosurgeon::reconcile(&mut right, &rsp).unwrap();

        // Merge in both directions on independent copies of each side.
        let mut l_then_r = left.fork();
        l_then_r.merge(&mut right.fork()).unwrap();
        let mut r_then_l = right.fork();
        r_then_l.merge(&mut left.fork()).unwrap();

        let merged_lr: SharedProject = autosurgeon::hydrate(&l_then_r).unwrap();
        let merged_rl: SharedProject = autosurgeon::hydrate(&r_then_l).unwrap();

        // Order-independent: both merge orders land on the identical document.
        assert_eq!(merged_lr, merged_rl);
        // Convergent: neither concurrent edit was lost.
        assert!(merged_lr.tags.contains(&"from-left".to_string()));
        assert!(merged_lr.tags.contains(&"from-right".to_string()));
        // And the base tags survived alongside the two new ones.
        assert!(merged_lr.tags.contains(&"RL".to_string()));
        assert_eq!(merged_lr.tags.len(), base.tags.len() + 2);
    }

    #[test]
    fn publish_does_not_mutate_canonical_db() {
        let (conn, pid) = seed();
        let dir = tempfile::tempdir().unwrap();
        let before = db_checksum(&conn);

        publish(&conn, dir.path(), pid).unwrap();
        // Re-publish to exercise the idempotent-overwrite path too.
        publish(&conn, dir.path(), pid).unwrap();

        assert_eq!(
            before,
            db_checksum(&conn),
            "publish must not write canonical tables"
        );
    }
}
