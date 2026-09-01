//! linxiv-share — Phase-0 quarantined CRDT store for "shared projects".
//!
//! A plain shared project is a read-only snapshot of one canonical project's subgraph
//! (project row + its papers/authors/tags + project notes + project tags), held
//! as an automerge document so a later phase can sync it peer-to-peer.
//! Publishing goes through core service APIs; its one canonical write is
//! `project::ensure_share_id` (persist the share uuid on first publish).
//! The documents are the whole store — one `<share_id>.automerge` file per
//! shared project under a share directory.
//!
//! The share directory is injected (`ShareStore::new(path)`); production resolves
//! it as `config::data_dir()/share`. Phase 1 adds a one-way iroh transport (see
//! `transport`) that serves every locally-published top-level doc to any peer
//! that knows its id, quarantining `received/` mirrors via an existence-based
//! access check. Plain shares carry no per-share secret or capability.
//! With the `sync-beelay` feature, e2ee shares live under `share/e2ee/` and
//! sync over beelay with capability-based membership via keyhive.
//!
//! ## Import (reader leg)
//! `import_shared_project` is a second, additive-only write path that merges a
//! received `SharedProject` into the canonical DB: notes and annotations match
//! by uuid, papers by source_id, the project by SHARE_ID (created when absent).
//! NOTE: Remote deletions are not propagated — the import surface is
//! append+update only.

mod model;
mod transport;

use std::path::{Path, PathBuf};

// `save` hands back a doc, so callers need to be able to name `AutoCommit`.
pub use automerge::AutoCommit;
use automerge::ReadDoc;
use rusqlite::Connection;

use linxiv_core::error::CoreError;
use linxiv_core::models::{
    validate_anchor, AnnotationIn, AnnotationUpdateIn, NoteIn, NoteUpdateIn, PaperMetadata,
    ProjectIn, ProjectUpdateIn,
};
use linxiv_core::service::{
    annotation as annotation_svc, author as author_svc, note as note_svc, paper as paper_svc,
    project as project_svc,
};

pub use linxiv_p2p::CustomRelay;
pub use model::{SharedAnnotation, SharedNote, SharedPaper, SharedProject, SharedSummary};
#[cfg(feature = "sync-beelay")]
pub use transport::{e2ee_dir, e2ee_received_dir, member_id_from_hex, member_id_hex};
pub use transport::{received_dir, valid_share_id, ShareNode, ShareTicket, ALPN};
#[cfg(feature = "sync-beelay")]
pub use transport::{E2eeSyncOutcome, MemberId, ProjectInvite, Role};

const SHARE_EXT: &str = "automerge";
const MAX_SHARED_TEXT: usize = 256 * 1024;

fn truncate_text(s: &str) -> String {
    if s.len() <= MAX_SHARED_TEXT {
        s.to_string()
    } else {
        s.chars()
            .scan(0, |len, c| {
                let new_len = *len + c.len_utf8();
                if new_len <= MAX_SHARED_TEXT {
                    *len = new_len;
                    Some(c)
                } else {
                    None
                }
            })
            .collect()
    }
}

/// Lowercase hyphenated uuid form (8-4-4-4-12 hex) — the form core mints and stores.
fn canonical_uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => matches!(b, b'0'..=b'9' | b'a'..=b'f'),
        })
}

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
    /// blob exceeds the byte cap.
    #[error("too large: {0}")]
    TooLarge(String),
    /// invite-time conflict: the member already holds a different role.
    #[error("member already holds a different role; revoke them first, then re-invite")]
    RoleConflict,
    /// role change refused: it would remove the project's last reader.
    #[error("role change would remove the project's last reader")]
    LastReader,
}

pub type Result<T> = std::result::Result<T, ShareError>;

// ── canonical → CRDT projection ─────────────────────────────────────────────

/// Gather a canonical project's subgraph and project it onto the CRDT model.
/// Missing project → `ShareError::NotFound`. The one canonical write is
/// `project::ensure_share_id`, which persists the share uuid on first publish.
pub fn build_shared_project(conn: &Connection, project_id: i64) -> Result<SharedProject> {
    let project = project_svc::get(
        conn,
        &project_svc::Project {
            project_fk: Some(project_id),
        },
    )?
    .ok_or_else(|| ShareError::NotFound(project_id.to_string()))?;
    let share_id = project_svc::ensure_share_id(conn, project_id)?;

    // Batched: this runs once per shared project on every background sync tick,
    // so the per-paper/per-note/per-annotation lookups it replaces were the
    // hottest N+1 in the app.
    let details = paper_svc::get_by_source_fks(conn, &project.source_fks)?;
    let paper_ids: Vec<i64> = details.iter().map(|p| p.paper_id).collect();
    let mut orcids = author_svc::paper_author_orcids(conn, &paper_ids)?;
    let mut papers = Vec::with_capacity(details.len());
    for p in details {
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

    let project_notes = note_svc::get_many(
        conn,
        &note_svc::Notes {
            project_fk: Some(project_id),
            ..Default::default()
        },
    )?;
    let project_anns = annotation_svc::get_many(
        conn,
        &annotation_svc::Annotations {
            project_fk: Some(project_id),
            ..Default::default()
        },
    )?;
    let fks: Vec<i64> = project_notes
        .iter()
        .map(|n| n.source_fk)
        .chain(project_anns.iter().map(|a| a.source_fk))
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let source_ids = paper_svc::source_ids_by_fk(conn, &fks)?;

    let mut notes = Vec::new();
    for n in project_notes {
        // Skip notes whose source_id no longer resolves (mirrors the annotation loop).
        let Some(paper_source_id) = source_ids.get(&n.source_fk) else {
            continue;
        };
        notes.push(SharedNote {
            uuid: n.uuid,
            paper_source_id: Some(paper_source_id.clone()),
            title: n.title,
            body: n.content,
            created_at: n.created_at.map(|t| t.to_string()),
            updated_at: n.updated_at.map(|t| t.to_string()),
        });
    }

    let mut annotations = Vec::new();
    for a in project_anns {
        // Skip annotations whose source_id no longer resolves (mirrors build_manifest).
        let Some(paper_source_id) = source_ids.get(&a.source_fk) else {
            continue;
        };
        annotations.push(SharedAnnotation {
            uuid: a.uuid,
            paper_source_id: paper_source_id.clone(),
            anchor: a.anchor,
            comment: a.comment,
            created_at: a.created_at.map(|t| t.to_string()),
            updated_at: a.updated_at.map(|t| t.to_string()),
        });
    }

    Ok(SharedProject {
        share_id,
        name: project.name,
        description: project.description,
        color: project.color.map(i64::from),
        tags: project.project_tags,
        papers,
        notes,
        annotations,
    })
}

// ── CRDT → canonical import (reader leg) ────────────────────────────────────

/// Project a `SharedPaper` onto the metadata write DTO. Parse the published
/// date if present; fall back to today like export_import's missing-date path.
fn paper_meta(p: &SharedPaper) -> PaperMetadata {
    let published = p
        .published
        .as_deref()
        .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        .unwrap_or_else(|| chrono::Utc::now().date_naive());
    PaperMetadata {
        source_id: p.source_id.clone(),
        version: p.version,
        title: truncate_text(&p.title),
        authors: p.authors.iter().map(|a| truncate_text(a)).collect(),
        published,
        updated: None,
        summary: truncate_text(&p.summary),
        category: None,
        categories: None,
        doi: None,
        journal_ref: None,
        comment: None,
        url: None,
        tags: (!p.tags.is_empty()).then(|| p.tags.iter().map(|t| truncate_text(t)).collect()),
        source: None,
        author_orcids: (!p.author_orcids.is_empty()).then(|| p.author_orcids.clone()),
    }
}

/// Merge a shared project into the canonical DB — additive + update only:
/// notes/annotations match by uuid, papers by source_id, the project by
/// SHARE_ID (created and linked when absent). Returns the linked project_fk.
// ponytail: no origin tracking, so remote deletions never propagate; upgrade
// when W4 editors exist.
pub fn import_shared_project(conn: &mut Connection, sp: &SharedProject) -> Result<i64> {
    if !canonical_uuid(&sp.share_id) {
        return Err(ShareError::Crdt(format!(
            "malformed share_id {:?}",
            sp.share_id
        )));
    }
    const MAX_SHARED_ITEMS: usize = 10_000;
    if sp.papers.len() > MAX_SHARED_ITEMS
        || sp.notes.len() > MAX_SHARED_ITEMS
        || sp.annotations.len() > MAX_SHARED_ITEMS
        || sp.tags.len() > MAX_SHARED_ITEMS
    {
        tracing::warn!(
            "share import: truncating oversized collections to {MAX_SHARED_ITEMS} items"
        );
    }
    let project_fk = match project_svc::find_by_share_id(conn, &sp.share_id)? {
        Some(fk) => {
            let existing = project_svc::get(
                conn,
                &project_svc::Project {
                    project_fk: Some(fk),
                },
            )?
            .ok_or(CoreError::ProjectNotFound(fk))?;
            // Union so remote tag removals stay local-only (additive rule).
            let mut tags = existing.project_tags.clone();
            for t in sp.tags.iter().take(MAX_SHARED_ITEMS) {
                let truncated = truncate_text(t);
                if !tags.contains(&truncated) {
                    tags.push(truncated);
                }
            }
            let new_name = truncate_text(&sp.name);
            let new_desc = truncate_text(&sp.description);
            // Absent/out-of-range remote color maps to None = "no change" in ProjectUpdateIn.
            let color = sp
                .color
                .and_then(|c| i32::try_from(c).ok())
                .filter(|c| Some(*c) != existing.color);
            if new_name != existing.name
                || new_desc != existing.description
                || color.is_some()
                || tags != existing.project_tags
            {
                project_svc::update(
                    conn,
                    &ProjectUpdateIn {
                        project_fk: fk,
                        name: Some(new_name),
                        description: Some(new_desc),
                        color: color.map(Some),
                        project_tags: Some(tags),
                        status: None,
                    },
                )?;
            }
            fk
        }
        None => {
            let fk = project_svc::create(
                conn,
                &ProjectIn {
                    name: truncate_text(&sp.name),
                    description: truncate_text(&sp.description),
                    color: sp.color.and_then(|c| i32::try_from(c).ok()),
                    tags: sp
                        .tags
                        .iter()
                        .take(MAX_SHARED_ITEMS)
                        .map(|t| truncate_text(t))
                        .collect(),
                    source_fks: Vec::new(),
                },
            )?;
            // Adopt the remote share uuid onto the fresh row's NULL SHARE_ID.
            project_svc::adopt_share_id(conn, fk, &sp.share_id)?;
            fk
        }
    };

    let mut linked: Vec<String> = Vec::new();
    const MAX_SOURCE_ID: usize = 512;
    // Deleted/known status is resolved in two bulk queries up front instead of
    // two queries per paper; nothing in the loop trashes papers, and `known`
    // is kept current across duplicate source_ids by inserting after a save.
    let candidate_ids: Vec<String> = sp
        .papers
        .iter()
        .take(MAX_SHARED_ITEMS)
        .filter(|p| !p.source_id.is_empty() && p.source_id.len() <= MAX_SOURCE_ID)
        .map(|p| p.source_id.clone())
        .collect();
    let trashed = paper_svc::deleted_source_ids(conn, &candidate_ids)?;
    let mut known: std::collections::HashSet<String> =
        paper_svc::existing_source_ids(conn, &candidate_ids)?
            .into_iter()
            .collect();
    for p in sp.papers.iter().take(MAX_SHARED_ITEMS) {
        // Identity field: skipped, never truncated.
        if p.source_id.is_empty() || p.source_id.len() > MAX_SOURCE_ID {
            tracing::warn!(
                "share import: skipping paper with invalid source_id ({} bytes)",
                p.source_id.len()
            );
            continue;
        }
        // A locally-trashed paper stays trashed.
        if trashed.contains(&p.source_id) {
            continue;
        }
        if !known.contains(&p.source_id) {
            // Metadata writes apply only to papers not already in the DB.
            // This also creates the paper root, so linking below resolves.
            paper_svc::save_paper_metadata(conn, &paper_meta(p), None)?;
            known.insert(p.source_id.clone());
        } else if !p.tags.is_empty() {
            let truncated_tags: Vec<_> = p.tags.iter().map(|t| truncate_text(t)).collect();
            paper_svc::add_paper_tags(conn, &p.source_id, &truncated_tags)?;
        }
        linked.push(p.source_id.clone());
    }
    if !linked.is_empty() {
        project_svc::link_imported(conn, project_fk, &linked)?;
    }
    let linked: std::collections::HashSet<&str> = linked.iter().map(String::as_str).collect();

    let existing_notes = note_svc::get_many(
        conn,
        &note_svc::Notes {
            project_fk: Some(project_fk),
            ..Default::default()
        },
    )?;
    let notes_by_uuid: std::collections::HashMap<&str, _> = existing_notes
        .iter()
        .map(|e| (e.uuid.as_str(), e))
        .collect();
    for n in sp.notes.iter().take(MAX_SHARED_ITEMS) {
        if let Some(e) = notes_by_uuid.get(n.uuid.as_str()) {
            // Skip when the local row was edited more recently than the remote entry.
            let local_newer = match (e.updated_at.map(|t| t.to_string()), &n.updated_at) {
                (Some(local), Some(remote)) => &local > remote,
                _ => false,
            };
            if local_newer {
                continue;
            }
            let title = truncate_text(&n.title);
            let content = truncate_text(&n.body);
            if e.title != title || e.content != content {
                note_svc::update(
                    conn,
                    &NoteUpdateIn {
                        note_id: e.note_id,
                        title: Some(title),
                        content: Some(content),
                    },
                )?;
            }
        } else {
            if !canonical_uuid(&n.uuid) {
                tracing::warn!(
                    "share import: skipping note with malformed uuid {:?}",
                    n.uuid
                );
                continue;
            }
            if note_svc::uuid_taken(conn, &n.uuid)? {
                tracing::warn!(
                    "share import: skipping note, uuid {} taken outside this project",
                    n.uuid
                );
                continue;
            }
            // Creatable only when the doc names a paper this import linked.
            let Some(sid) = n.paper_source_id.as_deref() else {
                continue;
            };
            if !linked.contains(sid) {
                continue;
            }
            let source_fk = paper_svc::ensure_paper_root(conn, sid)?;
            note_svc::create(
                conn,
                &NoteIn {
                    source_fk,
                    title: truncate_text(&n.title),
                    content: truncate_text(&n.body),
                    paper_id: None,
                    project_fk: Some(project_fk),
                    uuid: Some(n.uuid.clone()),
                },
            )?;
        }
    }

    let existing_anns = annotation_svc::get_many(
        conn,
        &annotation_svc::Annotations {
            project_fk: Some(project_fk),
            ..Default::default()
        },
    )?;
    let anns_by_uuid: std::collections::HashMap<&str, _> =
        existing_anns.iter().map(|e| (e.uuid.as_str(), e)).collect();
    for a in sp.annotations.iter().take(MAX_SHARED_ITEMS) {
        if let Some(e) = anns_by_uuid.get(a.uuid.as_str()) {
            // Skip when the local row was edited more recently than the remote entry.
            let local_newer = match (e.updated_at.map(|t| t.to_string()), &a.updated_at) {
                (Some(local), Some(remote)) => &local > remote,
                _ => false,
            };
            if local_newer {
                continue;
            }
            // The anchor is canonically immutable; only the comment updates.
            let comment = truncate_text(&a.comment);
            if e.comment != comment {
                annotation_svc::update(
                    conn,
                    &AnnotationUpdateIn {
                        annotation_id: e.annotation_id,
                        comment,
                    },
                )?;
            }
        } else {
            if !canonical_uuid(&a.uuid) {
                tracing::warn!(
                    "share import: skipping annotation with malformed uuid {:?}",
                    a.uuid
                );
                continue;
            }
            if annotation_svc::uuid_taken(conn, &a.uuid)? {
                tracing::warn!(
                    "share import: skipping annotation, uuid {} taken outside this project",
                    a.uuid
                );
                continue;
            }
            // Skip-not-fail on a bad anchor or an unlinked paper (export_import parity).
            if validate_anchor(&a.anchor).is_err() || !linked.contains(a.paper_source_id.as_str()) {
                continue;
            }
            let source_fk = paper_svc::ensure_paper_root(conn, &a.paper_source_id)?;
            annotation_svc::create(
                conn,
                &AnnotationIn {
                    source_fk,
                    anchor: a.anchor.clone(),
                    comment: truncate_text(&a.comment),
                    project_fk: Some(project_fk),
                    uuid: Some(a.uuid.clone()),
                },
            )?;
        }
    }
    Ok(project_fk)
}

// ── persistence (the store) ─────────────────────────────────────────────────

/// Path of a published doc: `share_dir/<share_id>.automerge`.
pub fn doc_path(share_dir: &Path, share_id: &str) -> PathBuf {
    share_dir.join(format!("{share_id}.{SHARE_EXT}"))
}

fn crdt<E: std::fmt::Display>(e: E) -> ShareError {
    ShareError::Crdt(e.to_string())
}

/// Reconcile `sp` into `<share_id>.automerge`, EVOLVING the existing doc when one
/// is on disk (so republish extends CRDT history instead of rebuilding it); a
/// missing or unloadable doc falls back to a fresh one (corrupt-skip spirit).
/// Returns the reconciled doc so callers can register it in the p2p registry
/// without re-reading and re-parsing the file just written.
pub fn save(share_dir: &Path, sp: &SharedProject) -> Result<AutoCommit> {
    std::fs::create_dir_all(share_dir)?;
    let final_path = doc_path(share_dir, &sp.share_id);
    let mut doc = std::fs::read(&final_path)
        .ok()
        .and_then(|bytes| AutoCommit::load(&bytes).ok())
        .unwrap_or_default();
    let before = doc.get_heads();
    autosurgeon::reconcile(&mut doc, sp).map_err(crdt)?;
    // No-op reconcile (heads unchanged) with the file already on disk: skip the write.
    if doc.get_heads() == before && final_path.is_file() {
        return Ok(doc);
    }
    // Write to a sibling temp file then rename.
    let tmp_path = share_dir.join(format!("{}.{SHARE_EXT}.tmp", sp.share_id));
    std::fs::write(&tmp_path, doc.save())?;
    std::fs::rename(&tmp_path, &final_path)?;
    Ok(doc)
}

/// Load `<share_id>.automerge` and hydrate it back into a `SharedProject`.
/// Missing file, or a doc with no content yet → `ShareError::NotFound`.
pub fn load(share_dir: &Path, share_id: &str) -> Result<SharedProject> {
    let bytes = match std::fs::read(doc_path(share_dir, share_id)) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ShareError::NotFound(share_id.to_string()))
        }
        Err(e) => return Err(e.into()),
    };
    let mut doc = AutoCommit::load(&bytes).map_err(crdt)?;
    // An empty doc has none of the keys hydrate requires, so hydrating it fails
    // with "unexpected None". E2ee mirrors are written as empty placeholders
    // before the first sync lands; that is "nothing here yet", not a CRDT fault.
    if doc.get_heads().is_empty() {
        return Err(ShareError::NotFound(share_id.to_string()));
    }
    autosurgeon::hydrate(&doc).map_err(crdt)
}

/// Build + save a project's snapshot. Returns the persisted share_id (uuid v4,
/// minted on first publish). Re-publishing evolves the existing doc.
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
        let summary = match summarize(&path, share_id) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("share list: skipping unreadable doc {share_id}: {e}");
                continue;
            }
        };
        // Skip a doc whose hydrated id doesn't match its filename stem (e.g. a
        // fresh pre-first-sync e2ee mirror hydrates to an empty share_id).
        if summary.share_id != share_id {
            tracing::warn!("share list: skipping mismatched doc {share_id}");
            continue;
        }
        out.push(summary);
    }
    Ok(out)
}

/// A doc's listing summary read straight off the automerge document: only
/// `share_id`/`name` are hydrated and the subgraphs are counted by list length,
/// so listing never materializes paper summaries, note bodies, or annotation
/// anchors the way a full `load` does.
fn summarize(path: &Path, share_id: &str) -> Result<SharedSummary> {
    let mut doc = AutoCommit::load(&std::fs::read(path)?).map_err(crdt)?;
    // Empty pre-first-sync e2ee placeholder: nothing here yet (mirrors `load`).
    if doc.get_heads().is_empty() {
        return Err(ShareError::NotFound(share_id.to_string()));
    }
    #[derive(autosurgeon::Hydrate)]
    struct Meta {
        share_id: String,
        name: String,
    }
    let meta: Meta = autosurgeon::hydrate(&doc).map_err(crdt)?;
    Ok(SharedSummary {
        share_id: meta.share_id,
        name: meta.name,
        paper_count: list_len(&doc, "papers")?,
        note_count: list_len(&doc, "notes")?,
        annotation_count: list_len(&doc, "annotations")?,
        tag_count: list_len(&doc, "tags")?,
    })
}

/// Length of a top-level list in the doc; a missing or non-list key is a
/// malformed doc (the listing skips it), matching hydrate's failure on it.
fn list_len(doc: &AutoCommit, key: &str) -> Result<usize> {
    match doc.get(automerge::ROOT, key).map_err(crdt)? {
        Some((automerge::Value::Object(automerge::ObjType::List), id)) => Ok(doc.length(&id)),
        _ => Err(ShareError::Crdt(format!("doc has no {key} list"))),
    }
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
                uuid: None,
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
                uuid: None,
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
                uuid: None,
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
                uuid: None,
            },
        )
        .unwrap();

        (conn, project_id)
    }

    fn db_checksum(conn: &Connection) -> Vec<(String, i64)> {
        [
            "PAPER",
            "PAPER_ROOTS",
            "PROJECT",
            "PROJECT_TO_PAPER",
            "NOTE",
            "ANNOTATION",
            "TAG",
        ]
        .iter()
        .map(|t| {
            let sql = format!("SELECT COUNT(*) FROM {t}");
            (
                t.to_string(),
                conn.query_row(&sql, [], |r| r.get(0)).unwrap(),
            )
        })
        .collect()
    }

    #[test]
    fn publish_list_get_roundtrip_matches_seed() {
        let (conn, pid) = seed();
        let dir = tempfile::tempdir().unwrap();
        let store = ShareStore::new(dir.path());

        let share_id = store.publish(&conn, pid).unwrap();
        // Persisted uuid identity: stable across republish, not the project id.
        assert_eq!(share_id.len(), 36);
        assert_ne!(share_id, pid.to_string());
        assert_eq!(store.publish(&conn, pid).unwrap(), share_id);

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
    fn build_and_import_roundtrip_carries_author_orcids() {
        let (conn, pid) = seed();
        // "First" has authors Alice, Bob (seeded via seed()'s pin() helper); give
        // Alice an ORCID directly on the AUTHOR row (as the harvest path would).
        conn.execute(
            "UPDATE AUTHOR SET AUTHOR_ORCID = ? WHERE AUTHOR_FULL_NAME = ?",
            rusqlite::params!["0000-0001-2345-6789", "Alice"],
        )
        .unwrap();

        let sp = build_shared_project(&conn, pid).unwrap();
        let first = sp.papers.iter().find(|p| p.title == "First").unwrap();
        assert_eq!(first.authors, vec!["Alice", "Bob"]);
        assert_eq!(
            first.author_orcids,
            vec![Some("0000-0001-2345-6789".to_string()), None]
        );

        // Import into a fresh DB: AUTHOR_ORCID lands via the existing fill-if-null path.
        let mut conn2 = open_in_memory().unwrap();
        storage::init_db(&conn2).unwrap();
        import_shared_project(&mut conn2, &sp).unwrap();
        let orcid: Option<String> = conn2
            .query_row(
                "SELECT AUTHOR_ORCID FROM AUTHOR WHERE AUTHOR_FULL_NAME = ?",
                ["Alice"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orcid.as_deref(), Some("0000-0001-2345-6789"));
        let bob_orcid: Option<String> = conn2
            .query_row(
                "SELECT AUTHOR_ORCID FROM AUTHOR WHERE AUTHOR_FULL_NAME = ?",
                ["Bob"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bob_orcid, None);
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
    fn save_load_byte_roundtrip_preserves_a_populated_orcid() {
        let (conn, pid) = seed();
        conn.execute(
            "UPDATE AUTHOR SET AUTHOR_ORCID = ? WHERE AUTHOR_FULL_NAME = ?",
            rusqlite::params!["0000-0001-2345-6789", "Alice"],
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let built = build_shared_project(&conn, pid).unwrap();

        save(dir.path(), &built).unwrap();
        let loaded = load(dir.path(), &built.share_id).unwrap();
        let first = loaded.papers.iter().find(|p| p.title == "First").unwrap();
        assert_eq!(
            first.author_orcids,
            vec![Some("0000-0001-2345-6789".to_string()), None]
        );
    }

    #[test]
    fn missing_project_is_not_found() {
        let (conn, _pid) = seed();
        match build_shared_project(&conn, 9999) {
            Err(ShareError::NotFound(id)) => assert_eq!(id, "9999"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    /// An e2ee mirror written before its first sync holds an empty doc; loading
    /// it must be NotFound, not a "crdt error: unexpected None" 500.
    #[test]
    fn empty_doc_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let id = "11111111-2222-4333-8444-555555555555";
        std::fs::write(doc_path(dir.path(), id), AutoCommit::new().save()).unwrap();
        match load(dir.path(), id) {
            Err(ShareError::NotFound(got)) => assert_eq!(got, id),
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
        let share_id = publish(&conn, dir.path(), project_id).unwrap();
        // A garbage doc beside the valid one must not break the whole listing.
        std::fs::write(doc_path(dir.path(), "999"), b"not a valid automerge doc").unwrap();
        // Nor an empty pre-first-sync e2ee placeholder doc.
        let placeholder = "11111111-2222-4333-8444-555555555555";
        std::fs::write(doc_path(dir.path(), placeholder), AutoCommit::new().save()).unwrap();
        let listed = list_shared(dir.path()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].share_id, share_id);
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
    fn publish_adds_no_rows_and_only_writes_share_id() {
        let (conn, pid) = seed();
        let dir = tempfile::tempdir().unwrap();
        let before = db_checksum(&conn);
        let row = |conn: &Connection| -> (Option<String>, String, String) {
            conn.query_row(
                "SELECT SHARE_ID, NAME, DESCRIPTION FROM PROJECT WHERE PROJECT_FK = ?1",
                [pid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
        };
        assert_eq!(row(&conn).0, None);

        let share_id = publish(&conn, dir.path(), pid).unwrap();
        // Re-publish to exercise the idempotent-overwrite path too.
        publish(&conn, dir.path(), pid).unwrap();

        assert_eq!(before, db_checksum(&conn));
        let (stored, name, description) = row(&conn);
        assert_eq!(stored.as_deref(), Some(share_id.as_str()));
        assert_eq!(name, "My Project");
        assert_eq!(description, "a project");
    }

    // Pins the paper-loop semantics of import_shared_project: a locally-trashed
    // paper stays trashed and unlinked (its notes/annotations skipped), a known
    // paper keeps its local metadata but gains the remote tags, and an unknown
    // paper is created from the remote metadata.
    #[test]
    fn import_skips_trashed_tags_known_and_creates_unknown_papers() {
        let (conn, pid) = seed();
        let mut sp = build_shared_project(&conn, pid).unwrap();
        sp.papers.push(SharedPaper {
            source_id: "arxiv:3".into(),
            version: 1,
            published: Some("2024-02-02".into()),
            title: "Third".into(),
            summary: "s3".into(),
            authors: vec!["Dan".into()],
            tags: vec!["new".into()],
            pdf_blob: None,
            author_orcids: Vec::new(),
        });

        // Target DB: arxiv:1 exists but is locally trashed, arxiv:2 exists
        // active with different local metadata, arxiv:3 is unknown.
        let mut conn2 = open_in_memory().unwrap();
        storage::init_db(&conn2).unwrap();
        let pin = |sid: &str, title: &str| PaperIn {
            title: title.into(),
            published: NaiveDate::from_ymd_opt(2023, 6, 1).unwrap(),
            source_id: Some(sid.into()),
            version: None,
            authors: Some(vec!["Local".into()]),
            summary: Some("local summary".into()),
            category: Some("cs.LG".into()),
            doi: None,
            url: None,
            tags: None,
            source: Some("arxiv".into()),
        };
        paper_svc::upsert(&mut conn2, &pin("arxiv:1", "Local First"), None).unwrap();
        paper_svc::upsert(&mut conn2, &pin("arxiv:2", "Local Second"), None).unwrap();
        paper_svc::delete(
            &mut conn2,
            &paper_svc::PaperRef::source("arxiv:1".to_string()),
        )
        .unwrap();

        let pfk = import_shared_project(&mut conn2, &sp).unwrap();

        // Trashed stays trashed and is not linked to the imported project.
        assert!(paper_svc::is_paper_deleted(&conn2, "arxiv:1").unwrap());
        let mut linked: Vec<String> = conn2
            .prepare(
                "SELECT r.SOURCE_ID FROM PROJECT_TO_PAPER pp \
                 JOIN PAPER_ROOTS r ON r.SOURCE_FK = pp.SOURCE_FK \
                 WHERE pp.PROJECT_FK = ?",
            )
            .unwrap()
            .query_map([pfk], |r| r.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        linked.sort();
        assert_eq!(linked, vec!["arxiv:2".to_string(), "arxiv:3".to_string()]);

        // Known paper: local metadata kept, remote tags merged in.
        let p2 = paper_svc::get(&conn2, &paper_svc::PaperRef::source("arxiv:2".to_string()))
            .unwrap()
            .unwrap();
        assert_eq!(p2.title, "Local Second");
        assert!(p2.tags.contains(&"vision".to_string()));

        // Unknown paper: created from the remote metadata.
        let p3 = paper_svc::get(&conn2, &paper_svc::PaperRef::source("arxiv:3".to_string()))
            .unwrap()
            .unwrap();
        assert_eq!(p3.title, "Third");

        // Only the note/annotation hanging off a linked paper come across:
        // note B (arxiv:2) lands, note A and the annotation (arxiv:1) do not.
        let notes = note_svc::get_many(
            &conn2,
            &note_svc::Notes {
                project_fk: Some(pfk),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].content, "body B");
        let anns = annotation_svc::get_many(
            &conn2,
            &annotation_svc::Annotations {
                project_fk: Some(pfk),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(anns.is_empty());
    }
}
