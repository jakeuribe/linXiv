//! project service — Rust port of `service/project.py`.
//!
//! Thin orchestration over `storage::queries::{project,tag,note,paper}`. DB-touching
//! fns take `conn` first (DI seam — never open from config). The `Project`/`Projects`
//! query objects live in `service/project.py` itself, so they stay
//! local here too; `ProjectIn`/`ProjectUpdateIn` are the shared *In DTOs (models.rs).
//!
//! Two write contracts that are deliberately opposite (do NOT unify):
//!   * `create` is ATOMIC — insert + membership in ONE transaction; a mid-membership
//!     failure rolls the PROJECT row back too (Python `Project.save()` on insert).
//!   * `update` is intentionally NON-atomic — tag sync and the field UPDATE are
//!     separate transactions, so a failure between them leaves tags changed and
//!     fields not (Python's documented three-transaction partial-failure semantics).

use std::collections::HashSet;

use chrono::{Duration, Utc};
use rusqlite::Connection;

use crate::error::{CoreError, Result};
use crate::models::{PaperDetails, ProjectDetails, ProjectIn, ProjectOut, ProjectUpdateIn, Status};
use crate::storage::db::transaction;
use crate::storage::queries::{paper as paperq, project as pq, tag as tq};
use crate::storage::query::{self, Q};

/// `service/project.py::Project` — single-project lookup key. `None` short-circuits
/// to a None/no-op result, matching Python's `if project.project_fk is None`.
#[derive(Debug, Clone, Default)]
pub struct Project {
    pub project_fk: Option<i64>,
}

/// `service/project.py::Projects` — multi-project filter (any combination).
#[derive(Debug, Clone, Default)]
pub struct Projects {
    pub project_fks: Option<Vec<i64>>,
    pub status: Option<Status>,
}

// ── Tag helpers (shared by create/update) — Python `_normalize_tags`/`_sync_tags`. ──

/// Strip + case-insensitive dedup, case-preserving, dropping blanks.
fn normalize_tags(tags: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for t in tags {
        let label = t.trim();
        if !label.is_empty() && seen.insert(label.to_lowercase()) {
            out.push(label.to_string());
        }
    }
    out
}

/// Diff-based tag sync: remove dropped, add new. Each storage call is its own
/// transaction (Python parity — a failure between them is partial).
fn sync_tags(conn: &mut Connection, project_id: i64, new_tags: &[String]) -> Result<()> {
    let normalized = normalize_tags(new_tags);
    let current = tq::get_project_tags(conn, project_id)?;
    let new_lower: HashSet<String> = normalized.iter().map(|t| t.to_lowercase()).collect();
    let cur_lower: HashSet<String> = current.iter().map(|t| t.to_lowercase()).collect();
    let to_remove: Vec<String> = current
        .iter()
        .filter(|t| !new_lower.contains(&t.to_lowercase()))
        .cloned()
        .collect();
    let to_add: Vec<String> = normalized
        .iter()
        .filter(|t| !cur_lower.contains(&t.to_lowercase()))
        .cloned()
        .collect();
    if !to_remove.is_empty() {
        tq::remove_project_tags(conn, project_id, &to_remove)?;
    }
    if !to_add.is_empty() {
        tq::add_project_tags(conn, project_id, &to_add)?;
    }
    Ok(())
}

/// `service/project.py::_to_details` — storage row → details, filling `project_tags`
/// (the storage read leaves them empty; `source_fks` are already populated).
fn fill_tags(conn: &Connection, mut p: ProjectDetails) -> Result<ProjectDetails> {
    if let Some(id) = p.id {
        p.project_tags = tq::get_project_tags(conn, id)?;
    }
    Ok(p)
}

/// Batched [`fill_tags`] for list paths: one tag query for the whole slice
/// instead of one per project.
fn fill_tags_bulk(conn: &Connection, projects: &mut [ProjectDetails]) -> Result<()> {
    let ids: Vec<i64> = projects.iter().filter_map(|p| p.id).collect();
    let mut by_project = tq::project_tags_by_project(conn, &ids)?;
    for p in projects {
        if let Some(id) = p.id {
            p.project_tags = by_project.remove(&id).unwrap_or_default();
        }
    }
    Ok(())
}

// ── Reads ─────────────────────────────────────────────────────────────────────

/// `service/project.py::get` — single project by project_fk (with tags). `None`
/// when project_fk is unset or no row matches.
pub fn get(conn: &Connection, project: &Project) -> Result<Option<ProjectDetails>> {
    let Some(fk) = project.project_fk else {
        return Ok(None);
    };
    pq::get_project(conn, fk, true)?
        .map(|p| fill_tags(conn, p))
        .transpose()
}

/// `get` by bare project_fk where absence is an error: the one place the
/// not-found contract comes from (`CoreError::ProjectNotFound` — route 404,
/// CLI exit 1, MCP tool error all word it identically).
pub fn get_required(conn: &Connection, project_fk: i64) -> Result<ProjectDetails> {
    get(
        conn,
        &Project {
            project_fk: Some(project_fk),
        },
    )?
    .ok_or(CoreError::ProjectNotFound(project_fk))
}

/// Existence-only [`get_required`] (same any-status semantics and
/// `ProjectNotFound` wording) for guard sites that discard the row: one
/// SELECT instead of the row + membership + tags triple.
pub fn require(conn: &Connection, project_fk: i64) -> Result<()> {
    if pq::project_exists(conn, project_fk)? {
        Ok(())
    } else {
        Err(CoreError::ProjectNotFound(project_fk))
    }
}

/// Link `tags` to a project, creating any that don't exist yet. Guards existence
/// first so CLI and MCP stop each doing it by hand. Returns the resulting tags.
pub fn add_project_tags(
    conn: &mut Connection,
    project_fk: i64,
    tags: &[String],
) -> Result<Vec<String>> {
    require(conn, project_fk)?;
    tq::add_project_tags(conn, project_fk, tags)
}

/// Unlink `tags` from a project (the TAG rows themselves survive). Returns the
/// remaining tags. Symmetric with [`add_project_tags`].
pub fn remove_project_tags(
    conn: &mut Connection,
    project_fk: i64,
    tags: &[String],
) -> Result<Vec<String>> {
    require(conn, project_fk)?;
    tq::remove_project_tags(conn, project_fk, tags)
}

/// The single mapping point to the canonical wire shape (`models::ProjectOut`,
/// SERIALIZER 3): resolves `source_fks` to namespaced source ids and renders
/// `color` as `#rrggbb`. All three surfaces serialize projects through here.
pub fn to_out(conn: &Connection, p: ProjectDetails) -> Result<ProjectOut> {
    let mut out = to_out_many(conn, vec![p])?;
    out.pop()
        .ok_or_else(|| CoreError::Internal("to_out_many dropped its input".into()))
}

/// Batched [`to_out`] for list paths: resolves every project's `source_fks` in
/// one chunked lookup instead of one query per project. Order and per-project
/// semantics (input-order source_ids, dropped unknown fks) match `to_out`.
pub fn to_out_many(conn: &Connection, projects: Vec<ProjectDetails>) -> Result<Vec<ProjectOut>> {
    let mut seen = HashSet::new();
    let all_fks: Vec<i64> = projects
        .iter()
        .flat_map(|p| p.source_fks.iter().copied())
        .filter(|fk| seen.insert(*fk))
        .collect();
    let by_fk = crate::service::paper::source_ids_by_fk(conn, &all_fks)?;
    projects
        .into_iter()
        .map(|p| {
            let source_ids: Vec<String> = p
                .source_fks
                .iter()
                .filter_map(|fk| by_fk.get(fk).cloned())
                .collect();
            Ok(ProjectOut {
                id: p
                    .id
                    .ok_or_else(|| CoreError::Internal("Project has no id".into()))?,
                name: p.name,
                description: p.description,
                color_hex: p.color.map(color_to_hex),
                project_tags: p.project_tags,
                paper_count: source_ids.len(),
                source_ids,
                status: p.status,
                created_at: p.created_at,
                updated_at: p.updated_at,
                archived_at: p.archived_at,
                share_id: p.share_id,
            })
        })
        .collect()
}

/// `service/project.py::get_many` — projects matching any combination of the
/// `Projects` filter fields (with tags).
pub fn get_many(conn: &Connection, projects: &Projects) -> Result<Vec<ProjectDetails>> {
    let condition = [
        projects
            .project_fks
            .as_deref()
            .filter(|fks| !fks.is_empty())
            .map(|fks| query::_in("PROJECT_FK", fks.iter().copied())),
        projects
            .status
            .map(|s| Q::new("STATUS = ?", pq::status_to_sql(s))),
    ]
    .into_iter()
    .flatten()
    .reduce(Q::and);
    let mut projects = pq::list_projects(conn, condition, true)?;
    fill_tags_bulk(conn, &mut projects)?;
    Ok(projects)
}

/// Persisted share identity: generate + store a uuid v4 on first call, return the
/// existing one afterwards. Errors if the project is absent or trashed.
pub fn ensure_share_id(conn: &Connection, project_fk: i64) -> Result<String> {
    ensure_membership_writable(conn, project_fk)?;
    let candidate = uuid::Uuid::new_v4().to_string();
    pq::ensure_share_id(conn, project_fk, &candidate)?.ok_or(CoreError::ProjectNotFound(project_fk))
}

/// Reverse share lookup: the project (if any) whose SHARE_ID equals `share_id`.
pub fn find_by_share_id(conn: &Connection, share_id: &str) -> Result<Option<i64>> {
    pq::find_by_share_id(conn, share_id)
}

/// Detach the local project (if any) linked to this share_id; the project and
/// the share both survive. Returns whether a link existed.
pub fn release_share_id(conn: &Connection, share_id: &str) -> Result<bool> {
    Ok(pq::release_share_id(conn, share_id)? > 0)
}

/// Adopt share_id for this project; errors if another live project claims it.
pub fn adopt_share_id(conn: &Connection, project_fk: i64, share_id: &str) -> Result<()> {
    pq::release_share_id_from_deleted(conn, share_id)?;
    if pq::share_id_claimed_by_other(conn, project_fk, share_id)? {
        return Err(CoreError::ProjectImport(format!(
            "share id {share_id} already claimed by another live project"
        )));
    }
    let stored = pq::ensure_share_id(conn, project_fk, share_id)?
        .ok_or(CoreError::ProjectNotFound(project_fk))?;
    if stored != share_id {
        tracing::warn!(
            "adopt_share_id: project {project_fk} already has SHARE_ID {stored}; archive share id {share_id} not adopted"
        );
    }
    Ok(())
}

// ── Create (ATOMIC insert + membership) ─────────────────────────────────────────

/// `service/project.py::create` — insert a new project and its membership in ONE
/// transaction, then link tags (separate, like Python `save()` then `add_project_tags`).
///
/// Atomicity is load-bearing: `insert_project` and `save_source_fks` run in the same
/// `storage::db::transaction`, so a mid-membership failure (e.g. a SOURCE_FK with no
/// PAPER_ROOTS parent) rolls the PROJECT row back too — no orphan project.
pub fn create(conn: &mut Connection, project: &ProjectIn) -> Result<i64> {
    let name = project.name.trim();
    if name.is_empty() {
        return Err(CoreError::Validation("name cannot be blank".into()));
    }
    let id = transaction(conn, |tx| {
        let id = pq::insert_project(
            tx,
            name,
            &project.description,
            project.color,
            Status::Active,
            None,
        )?;
        pq::save_source_fks(tx, id, &project.source_fks)?;
        Ok(id)
    })?;
    // Tags after the insert tx — Python adds them once the project has an id.
    if !project.tags.is_empty() {
        tq::add_project_tags(conn, id, &project.tags)?;
    }
    Ok(id)
}

// ── Update (partial; UNSET colour sentinel; NON-atomic by design) ───────────────

/// `service/project.py::update` — partial update. `color` is the D16 UNSET sentinel
/// (`Option<Option<i32>>`): outer `None` = unchanged, `Some(None)` = clear, `Some(Some(v))`
/// = set. Returns `ProjectNotFound` if absent, `ProjectDeleted` if soft-deleted and the
/// update is not a restore (status=Active).
///
/// Three-transaction partial-failure semantics (intentional, do NOT make atomic): tag
/// sync and the field UPDATE are separate transactions, so a failure between them leaves
/// tags changed and fields not.
pub fn update(conn: &mut Connection, upd: &ProjectUpdateIn) -> Result<()> {
    let mut p = pq::get_project(conn, upd.project_fk, false)?
        .ok_or(CoreError::ProjectNotFound(upd.project_fk))?;
    if p.status == Status::Deleted && upd.status != Some(Status::Active) {
        return Err(CoreError::ProjectDeleted(
            "cannot update a deleted project".into(),
        ));
    }
    let mut dirty = false;
    if let Some(name) = &upd.name {
        let stripped = name.trim();
        if stripped.is_empty() {
            return Err(CoreError::Validation("name cannot be blank".into()));
        }
        p.name = stripped.to_string();
        dirty = true;
    }
    if let Some(description) = &upd.description {
        p.description = description.clone();
        dirty = true;
    }
    if let Some(color) = upd.color {
        // outer Some = caller supplied a value (inner None clears, inner Some sets).
        p.color = color;
        dirty = true;
    }
    if let Some(tags) = &upd.project_tags {
        let id =
            p.id.ok_or_else(|| CoreError::Internal("Project has no id after fetch".into()))?;
        // Separate transaction; bumps dirty so UPDATED_AT reflects the tag change.
        sync_tags(conn, id, tags)?;
        dirty = true;
    }

    if let Some(status) = upd.status {
        if status == p.status {
            if dirty {
                save_fields(conn, &p)?;
            }
        } else {
            // A status transition always writes (archive/restore/delete persist the
            // field mutations above too). archived_at: archive/trash stamp now; restore clears.
            let archived_at = match status {
                Status::Archived | Status::Deleted => Some(Utc::now().naive_utc()),
                Status::Active => None,
            };
            pq::update_project_fields(
                conn,
                upd.project_fk,
                &p.name,
                &p.description,
                p.color,
                status,
                archived_at,
            )?;
        }
        return Ok(());
    }
    if dirty {
        save_fields(conn, &p)?;
    }
    Ok(())
}

/// Fields-only save preserving the loaded status/archived_at (Python `p.save()`).
fn save_fields(conn: &Connection, p: &ProjectDetails) -> Result<()> {
    let fk =
        p.id.ok_or_else(|| CoreError::Internal("Project has no id".into()))?;
    pq::update_project_fields(
        conn,
        fk,
        &p.name,
        &p.description,
        p.color,
        p.status,
        p.archived_at,
    )?;
    Ok(())
}

// ── Membership seam (Python `add_papers`/`remove_papers`/`link_imported`) ───────

/// `service/project.py::ensure_membership_writable` — guards only (existence +
/// not-deleted), no write. Import flows call this before mutating the library.
pub fn ensure_membership_writable(conn: &Connection, project_fk: i64) -> Result<()> {
    match pq::get_project(conn, project_fk, false)? {
        None => Err(CoreError::ProjectNotFound(project_fk)),
        Some(p) if p.status == Status::Deleted => Err(CoreError::ProjectDeleted(
            "cannot update a deleted project".into(),
        )),
        Some(_) => Ok(()),
    }
}

/// `service/project.py::_resolve_source_ids` — paper ids → SOURCE_FKs. Ids are
/// stripped and deduped (keyed on the stripped form, reported once). Returns
/// (fks in first-seen order, unresolved ids verbatim). Trashed papers resolve —
/// `source_fks_by_id` has no status filter, matching `get_paper_root`.
fn resolve_source_ids(conn: &Connection, source_ids: &[String]) -> Result<(Vec<i64>, Vec<String>)> {
    let mut seen = HashSet::new();
    let deduped: Vec<(&str, &String)> = source_ids
        .iter()
        .filter_map(|sid| {
            let stripped = sid.trim();
            seen.insert(stripped.to_string()).then_some((stripped, sid))
        })
        .collect();
    let stripped: Vec<&str> = deduped.iter().map(|(s, _)| *s).collect();
    let by_id = paperq::source_fks_by_id(conn, &stripped)?;
    let mut fks = Vec::new();
    let mut failed = Vec::new();
    for (stripped, sid) in deduped {
        match by_id.get(stripped) {
            Some(fk) => fks.push(*fk),
            None => failed.push(sid.clone()),
        }
    }
    Ok((fks, failed))
}

/// `service/project.py::add_papers` — add by paper id (per-row inserts; never a full
/// rewrite). Unresolved ids are returned verbatim while the rest are still added.
/// Raises ProjectNotFound/ProjectDeleted per the membership guards.
pub fn add_papers(
    conn: &Connection,
    project_fk: i64,
    source_ids: &[String],
) -> Result<Vec<String>> {
    ensure_membership_writable(conn, project_fk)?;
    let (fks, failed) = resolve_source_ids(conn, source_ids)?;
    if !fks.is_empty() {
        pq::add_papers(conn, project_fk, &fks)?;
    }
    Ok(failed)
}

/// `service/project.py::remove_papers` — same contract as `add_papers`, per-row deletes.
pub fn remove_papers(
    conn: &Connection,
    project_fk: i64,
    source_ids: &[String],
) -> Result<Vec<String>> {
    ensure_membership_writable(conn, project_fk)?;
    let (fks, failed) = resolve_source_ids(conn, source_ids)?;
    if !fks.is_empty() {
        pq::remove_papers(conn, project_fk, &fks)?;
    }
    Ok(failed)
}

/// A project's papers for the text exporters, resolved ONE way for every
/// surface: the project's latest-version rows in library order (the shipped
/// GUI contract — app.py's `[p for p in list_paper_details(latest) if id in
/// ids]`), filtered in SQL rather than scanning the whole library.
pub fn export_papers(conn: &Connection, source_fks: &[i64]) -> Result<Vec<PaperDetails>> {
    crate::service::paper::get_by_source_fks(conn, source_fks)
}

/// Receipt for the single-paper membership ops — one shape for all three
/// surfaces (`ok` + ids + the project's post-op paper count).
#[derive(Debug, serde::Serialize, ts_rs::TS)]
pub struct PaperMembershipReceipt {
    pub ok: bool,
    pub project_id: i64,
    pub paper_id: String,
    pub paper_count: usize,
}

/// Add one paper to a project, returning the shared receipt. An id that
/// resolves to no paper root is `PaperNotFound` (guards per `add_papers`).
pub fn add_paper(
    conn: &Connection,
    project_fk: i64,
    source_id: &str,
) -> Result<PaperMembershipReceipt> {
    membership_receipt(conn, project_fk, source_id, add_papers)
}

/// Remove one paper from a project — same contract as [`add_paper`].
pub fn remove_paper(
    conn: &Connection,
    project_fk: i64,
    source_id: &str,
) -> Result<PaperMembershipReceipt> {
    membership_receipt(conn, project_fk, source_id, remove_papers)
}

fn membership_receipt(
    conn: &Connection,
    project_fk: i64,
    source_id: &str,
    op: fn(&Connection, i64, &[String]) -> Result<Vec<String>>,
) -> Result<PaperMembershipReceipt> {
    let failed = op(conn, project_fk, &[source_id.to_string()])?;
    if !failed.is_empty() {
        return Err(CoreError::PaperNotFound(source_id.to_string()));
    }
    let paper_count =
        pq::active_paper_count(conn, project_fk)?.ok_or(CoreError::ProjectNotFound(project_fk))?;
    Ok(PaperMembershipReceipt {
        ok: true,
        project_id: project_fk,
        paper_id: source_id.to_string(),
        paper_count,
    })
}

/// `service/project.py::link_imported` — same write path as `add_papers`, but ids come
/// from the import (not a user), so an unresolved id is logged, not returned.
pub fn link_imported(conn: &Connection, project_fk: i64, source_ids: &[String]) -> Result<()> {
    let failed = add_papers(conn, project_fk, source_ids)?;
    if !failed.is_empty() {
        tracing::warn!(
            "link_imported: {} of {} imported id(s) did not resolve for project {}: {:?}",
            failed.len(),
            source_ids.len(),
            project_fk,
            &failed[..failed.len().min(5)],
        );
    }
    Ok(())
}

/// `service/project.py::remove_paper_from_all_projects` — delete this paper's membership
/// rows across all projects (single transaction). Returns the PROJECT_FKs it was in.
pub use crate::storage::queries::project::remove_paper_from_all_projects;

/// `service/project.py::remove_paper_from_all_projects_by_id` — resolve a paper id
/// (stripped) then delete its membership everywhere. `None` if the id is unknown.
pub fn remove_paper_from_all_projects_by_id(
    conn: &mut Connection,
    source_id: &str,
) -> Result<Option<Vec<i64>>> {
    paperq::get_paper_root(conn, source_id.trim())?
        .map(|root| pq::remove_paper_from_all_projects(conn, root.source_fk))
        .transpose()
}

// ── Status transitions / trash ──────────────────────────────────────────────────

/// Load fields and write the target status (+archived_at). No-op if the project is
/// absent or project_fk is unset — Python's delete/restore/archive silently return.
fn set_status(conn: &Connection, project: &Project, target: Status) -> Result<()> {
    let Some(fk) = project.project_fk else {
        return Ok(());
    };
    let Some(p) = pq::get_project(conn, fk, false)? else {
        return Ok(());
    };
    let archived_at = match target {
        Status::Archived | Status::Deleted => Some(Utc::now().naive_utc()),
        Status::Active => None,
    };
    pq::update_project_fields(
        conn,
        fk,
        &p.name,
        &p.description,
        p.color,
        target,
        archived_at,
    )?;
    Ok(())
}

/// `service/project.py::delete` — soft-delete (trash).
pub fn delete(conn: &Connection, project: &Project) -> Result<()> {
    set_status(conn, project, Status::Deleted)
}

/// `service/project.py::restore` — un-trash / un-archive.
pub fn restore(conn: &Connection, project: &Project) -> Result<()> {
    set_status(conn, project, Status::Active)
}

/// `service/project.py::archive`.
pub fn archive(conn: &Connection, project: &Project) -> Result<()> {
    set_status(conn, project, Status::Archived)
}

/// Guard for trash-only operations (restore-from-trash, hard delete): the project
/// must exist and be soft-deleted. `restore`/`hard_delete` themselves stay
/// unguarded — `restore` also serves un-archive, which starts from `Archived`.
pub fn require_trashed(conn: &Connection, project_fk: i64) -> Result<()> {
    let Some(d) = get(
        conn,
        &Project {
            project_fk: Some(project_fk),
        },
    )?
    else {
        return Err(CoreError::ProjectNotFound(project_fk));
    };
    if d.status != Status::Deleted {
        return Err(CoreError::BadRequest(format!(
            "Project {project_fk} is not in trash."
        )));
    }
    Ok(())
}

/// `service/project.py::hard_delete` — permanent removal (+ associations). No-op if
/// project_fk is unset; the storage fn no-ops cleanly on an absent project.
pub fn hard_delete(conn: &mut Connection, project: &Project) -> Result<()> {
    if let Some(fk) = project.project_fk {
        pq::hard_delete_project(conn, fk)?;
    }
    Ok(())
}

/// `service/project.py::list_deleted` — soft-deleted projects, newest-trashed first
/// (archived_at desc; rows with no timestamp sort last, matching `datetime.min`).
pub fn list_deleted(conn: &Connection) -> Result<Vec<ProjectDetails>> {
    let mut projects = pq::list_projects(conn, Some(Q::new("STATUS = ?", "deleted")), true)?;
    projects.sort_by_key(|p| std::cmp::Reverse(p.archived_at));
    fill_tags_bulk(conn, &mut projects)?;
    Ok(projects)
}

/// `service/project.py::purge_old` — hard-delete projects trashed more than `days`
/// days ago. Returns the count purged.
pub fn purge_old(conn: &mut Connection, days: i64) -> Result<usize> {
    let cutoff = Utc::now().naive_utc() - Duration::days(days);
    let deleted = pq::list_projects(conn, Some(Q::new("STATUS = ?", "deleted")), false)?;
    let old: Vec<i64> = deleted
        .iter()
        .filter(|p| p.archived_at.is_some_and(|a| a < cutoff))
        .filter_map(|p| p.id)
        .collect();
    for fk in &old {
        pq::hard_delete_project(conn, *fk)?;
    }
    Ok(old.len())
}

// ── Colour helpers — Python `projects.py::color_to_hex`/`color_from_hex`. ──────

pub fn color_to_hex(color: i32) -> String {
    format!("#{color:06x}")
}

pub fn color_from_hex(hex: &str) -> Result<i32> {
    i32::from_str_radix(hex.trim_start_matches('#'), 16)
        .map_err(|e| CoreError::Internal(format!("bad colour hex {hex:?}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::db;

    fn setup() -> Connection {
        let conn = db();
        conn.execute_batch(
            "INSERT INTO PAPER_ROOTS (SOURCE_FK, SOURCE_ID, STATUS) VALUES
                 (10, 'arxiv:1', 'active'),
                 (11, 'arxiv:2', 'active'),
                 (12, 'arxiv:3', 'deleted');",
        )
        .unwrap();
        conn
    }

    fn pin(name: &str, source_fks: Vec<i64>, tags: Vec<&str>) -> ProjectIn {
        ProjectIn {
            name: name.into(),
            description: "d".into(),
            color: Some(255),
            tags: tags.into_iter().map(String::from).collect(),
            source_fks,
        }
    }

    #[test]
    fn create_inserts_membership_and_tags_atomically() {
        let mut conn = setup();
        let id = create(&mut conn, &pin("Proj", vec![10, 11], vec!["RL", "Vision"])).unwrap();

        let got = get(
            &conn,
            &Project {
                project_fk: Some(id),
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(got.name, "Proj");
        assert_eq!(got.color, Some(255));
        assert_eq!(got.source_fks, vec![10, 11]);
        assert_eq!(got.project_tags, vec!["RL", "Vision"]); // ORDER BY label
        assert_eq!(got.status, Status::Active);
    }

    #[test]
    fn membership_receipt_counts_and_pins_wire_shape() {
        let mut conn = setup();
        // fk 12 is a trashed root: linked but excluded from paper_count
        // (active-membership semantics, same as source_fks.len()).
        let id = create(&mut conn, &pin("Proj", vec![10, 12], vec![])).unwrap();

        let receipt = add_paper(&conn, id, "arxiv:2").unwrap();
        assert_eq!(
            serde_json::to_string(&receipt).unwrap(),
            format!(r#"{{"ok":true,"project_id":{id},"paper_id":"arxiv:2","paper_count":2}}"#)
        );
        let receipt = remove_paper(&conn, id, "arxiv:2").unwrap();
        assert_eq!(receipt.paper_count, 1);
        // Unresolvable id → the typed miss, nothing changed.
        assert!(matches!(
            add_paper(&conn, id, "arxiv:ghost").unwrap_err(),
            CoreError::PaperNotFound(sid) if sid == "arxiv:ghost"
        ));
    }

    #[test]
    fn require_matches_get_required_semantics() {
        let mut conn = setup();
        let id = create(&mut conn, &pin("Proj", vec![], vec![])).unwrap();
        require(&conn, id).unwrap();
        // Any-status, like get_required: a trashed project still resolves.
        delete(
            &conn,
            &Project {
                project_fk: Some(id),
            },
        )
        .unwrap();
        require(&conn, id).unwrap();
        assert!(matches!(
            require(&conn, 999).unwrap_err(),
            CoreError::ProjectNotFound(999)
        ));
    }

    /// Pins the batched list path (bulk membership, bulk tags, to_out_many)
    /// against the single-project path for every field, per project.
    #[test]
    fn get_many_and_to_out_many_match_single_paths() {
        let mut conn = setup();
        let a = create(&mut conn, &pin("A", vec![11, 10], vec!["RL", "Vision"])).unwrap();
        let b = create(&mut conn, &pin("B", vec![10], vec![])).unwrap();

        let many = get_many(&conn, &Projects::default()).unwrap();
        assert_eq!(many.len(), 2);
        for p in &many {
            let single = get(&conn, &Project { project_fk: p.id }).unwrap().unwrap();
            assert_eq!(p.source_fks, single.source_fks);
            assert_eq!(p.project_tags, single.project_tags);
        }
        let by_id = |id| many.iter().find(|p| p.id == Some(id)).unwrap();
        assert_eq!(by_id(a).source_fks, vec![11, 10]); // insertion order kept
        assert_eq!(by_id(b).project_tags, Vec::<String>::new());

        let singles: Vec<_> = many
            .iter()
            .map(|p| to_out(&conn, p.clone()).unwrap())
            .collect();
        let bulk = to_out_many(&conn, many).unwrap();
        assert_eq!(
            serde_json::to_value(&bulk).unwrap(),
            serde_json::to_value(&singles).unwrap()
        );
    }

    #[test]
    fn create_blank_name_rejected() {
        let mut conn = setup();
        assert!(matches!(
            create(&mut conn, &pin("   ", vec![], vec![])).unwrap_err(),
            CoreError::Validation(_)
        ));
    }

    #[test]
    fn create_rolls_back_project_on_membership_failure() {
        let mut conn = setup();
        // 999 has no PAPER_ROOTS parent → save_source_fks FK-violates mid-transaction.
        let err = create(&mut conn, &pin("Doomed", vec![10, 999], vec![])).unwrap_err();
        assert!(
            matches!(err, CoreError::Internal(_)),
            "FK violation surfaces as Internal"
        );
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM PROJECT", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            n, 0,
            "PROJECT row rolled back with the failed membership write"
        );
        let m: i64 = conn
            .query_row("SELECT COUNT(*) FROM PROJECT_TO_PAPER", [], |r| r.get(0))
            .unwrap();
        assert_eq!(m, 0, "the partial INSERT of fk 10 rolled back too");
    }

    #[test]
    fn update_color_sentinel_absent_clear_set() {
        let mut conn = setup();
        let id = create(&mut conn, &pin("P", vec![], vec![])).unwrap();

        // absent (outer None) → unchanged
        update(
            &mut conn,
            &ProjectUpdateIn {
                project_fk: id,
                name: Some("P2".into()),
                description: None,
                color: None,
                project_tags: None,
                status: None,
            },
        )
        .unwrap();
        let got = get(
            &conn,
            &Project {
                project_fk: Some(id),
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(got.name, "P2");
        assert_eq!(got.color, Some(255), "absent color left unchanged");

        // Some(None) → clear
        update(
            &mut conn,
            &ProjectUpdateIn {
                project_fk: id,
                name: None,
                description: None,
                color: Some(None),
                project_tags: None,
                status: None,
            },
        )
        .unwrap();
        assert_eq!(
            get(
                &conn,
                &Project {
                    project_fk: Some(id)
                }
            )
            .unwrap()
            .unwrap()
            .color,
            None
        );

        // Some(Some(7)) → set
        update(
            &mut conn,
            &ProjectUpdateIn {
                project_fk: id,
                name: None,
                description: None,
                color: Some(Some(7)),
                project_tags: None,
                status: None,
            },
        )
        .unwrap();
        assert_eq!(
            get(
                &conn,
                &Project {
                    project_fk: Some(id)
                }
            )
            .unwrap()
            .unwrap()
            .color,
            Some(7)
        );
    }

    #[test]
    fn update_status_transitions_and_deleted_guard() {
        let mut conn = setup();
        let id = create(&mut conn, &pin("P", vec![10], vec![])).unwrap();

        let with_status = |s: Status| ProjectUpdateIn {
            project_fk: id,
            name: None,
            description: None,
            color: None,
            project_tags: None,
            status: Some(s),
        };

        update(&mut conn, &with_status(Status::Archived)).unwrap();
        let got = get(
            &conn,
            &Project {
                project_fk: Some(id),
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(got.status, Status::Archived);
        assert!(got.archived_at.is_some());

        update(&mut conn, &with_status(Status::Active)).unwrap();
        let got = get(
            &conn,
            &Project {
                project_fk: Some(id),
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(got.status, Status::Active);
        assert!(got.archived_at.is_none(), "restore clears archived_at");
        assert_eq!(
            got.source_fks,
            vec![10],
            "transition leaves membership intact"
        );

        update(&mut conn, &with_status(Status::Deleted)).unwrap();
        assert_eq!(
            get(
                &conn,
                &Project {
                    project_fk: Some(id)
                }
            )
            .unwrap()
            .unwrap()
            .status,
            Status::Deleted
        );

        // deleted project: a non-restore update is rejected …
        assert!(matches!(
            update(
                &mut conn,
                &ProjectUpdateIn {
                    project_fk: id,
                    name: Some("nope".into()),
                    description: None,
                    color: None,
                    project_tags: None,
                    status: None,
                },
            )
            .unwrap_err(),
            CoreError::ProjectDeleted(_)
        ));
        // … but a restore (status=Active) is allowed.
        update(&mut conn, &with_status(Status::Active)).unwrap();
        assert_eq!(
            get(
                &conn,
                &Project {
                    project_fk: Some(id)
                }
            )
            .unwrap()
            .unwrap()
            .status,
            Status::Active
        );

        // missing project → not found
        assert!(matches!(
            update(
                &mut conn,
                &ProjectUpdateIn {
                    project_fk: 9999,
                    name: Some("x".into()),
                    description: None,
                    color: None,
                    project_tags: None,
                    status: None,
                },
            )
            .unwrap_err(),
            CoreError::ProjectNotFound(_)
        ));
    }

    #[test]
    fn update_syncs_tags_and_fields() {
        let mut conn = setup();
        let id = create(&mut conn, &pin("P", vec![], vec!["keep", "drop"])).unwrap();
        update(
            &mut conn,
            &ProjectUpdateIn {
                project_fk: id,
                name: Some("Renamed".into()),
                description: None,
                color: None,
                project_tags: Some(vec!["keep".into(), "new".into()]),
                status: None,
            },
        )
        .unwrap();
        let got = get(
            &conn,
            &Project {
                project_fk: Some(id),
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(got.name, "Renamed");
        assert_eq!(got.project_tags, vec!["keep", "new"]); // drop removed, new added
    }

    #[test]
    fn add_and_remove_papers_by_source_id() {
        let mut conn = setup();
        let id = create(&mut conn, &pin("P", vec![], vec![])).unwrap();

        // unknown id reported, known ones added (dedup of repeats).
        let failed = add_papers(
            &conn,
            id,
            &["arxiv:1".into(), "arxiv:1".into(), "nope".into()],
        )
        .unwrap();
        assert_eq!(failed, vec!["nope"]);
        assert_eq!(
            get(
                &conn,
                &Project {
                    project_fk: Some(id)
                }
            )
            .unwrap()
            .unwrap()
            .source_fks,
            vec![10]
        );

        // trashed paper (arxiv:3) resolves and the row is written, but project reads
        // hide it (active-only membership), so source_fks stays [10].
        assert!(add_papers(&conn, id, &["arxiv:3".into()])
            .unwrap()
            .is_empty());
        assert_eq!(
            get(
                &conn,
                &Project {
                    project_fk: Some(id)
                }
            )
            .unwrap()
            .unwrap()
            .source_fks,
            vec![10]
        );

        let failed = remove_papers(&conn, id, &["arxiv:1".into(), "ghost".into()]).unwrap();
        assert_eq!(failed, vec!["ghost"]);
        assert!(get(
            &conn,
            &Project {
                project_fk: Some(id)
            }
        )
        .unwrap()
        .unwrap()
        .source_fks
        .is_empty());
    }

    #[test]
    fn membership_guards() {
        let mut conn = setup();
        let id = create(&mut conn, &pin("P", vec![], vec![])).unwrap();
        delete(
            &conn,
            &Project {
                project_fk: Some(id),
            },
        )
        .unwrap();
        assert!(matches!(
            add_papers(&conn, id, &["arxiv:1".into()]).unwrap_err(),
            CoreError::ProjectDeleted(_)
        ));
        assert!(matches!(
            ensure_membership_writable(&conn, 9999).unwrap_err(),
            CoreError::ProjectNotFound(_)
        ));
    }

    #[test]
    fn link_imported_links_and_swallows_unresolved() {
        let mut conn = setup();
        let id = create(&mut conn, &pin("P", vec![], vec![])).unwrap();
        // unresolved id does not error (logged, not returned).
        link_imported(&conn, id, &["arxiv:1".into(), "missing".into()]).unwrap();
        assert_eq!(
            get(
                &conn,
                &Project {
                    project_fk: Some(id)
                }
            )
            .unwrap()
            .unwrap()
            .source_fks,
            vec![10]
        );
    }

    #[test]
    fn remove_paper_from_all_projects_by_id_resolves_or_none() {
        let mut conn = setup();
        let a = create(&mut conn, &pin("A", vec![10], vec![])).unwrap();
        let b = create(&mut conn, &pin("B", vec![10], vec![])).unwrap();
        let mut fks = remove_paper_from_all_projects_by_id(&mut conn, " arxiv:1 ")
            .unwrap()
            .unwrap();
        fks.sort();
        assert_eq!(fks, vec![a, b]);
        assert!(remove_paper_from_all_projects_by_id(&mut conn, "unknown")
            .unwrap()
            .is_none());
    }

    #[test]
    fn list_deleted_orders_newest_first_and_purge_old() {
        let mut conn = setup();
        let p1 = create(&mut conn, &pin("Old", vec![], vec![])).unwrap();
        let p2 = create(&mut conn, &pin("New", vec![], vec![])).unwrap();
        delete(
            &conn,
            &Project {
                project_fk: Some(p1),
            },
        )
        .unwrap();
        delete(
            &conn,
            &Project {
                project_fk: Some(p2),
            },
        )
        .unwrap();
        // Backdate p1's trash time so it is "old" and sorts last.
        conn.execute(
            "UPDATE PROJECT SET ARCHIVED_AT = '2000-01-01T00:00:00' WHERE PROJECT_FK = ?1",
            [p1],
        )
        .unwrap();

        let deleted = list_deleted(&conn).unwrap();
        assert_eq!(deleted.len(), 2);
        assert_eq!(deleted[0].id, Some(p2), "newest trashed first");
        assert_eq!(deleted[1].id, Some(p1));

        // purge_old removes only the backdated one.
        assert_eq!(purge_old(&mut conn, 30).unwrap(), 1);
        assert!(get(
            &conn,
            &Project {
                project_fk: Some(p1)
            }
        )
        .unwrap()
        .is_none());
        assert!(get(
            &conn,
            &Project {
                project_fk: Some(p2)
            }
        )
        .unwrap()
        .is_some());
    }

    #[test]
    fn get_many_and_get_projects_page() {
        let mut conn = setup();
        let a = create(&mut conn, &pin("Active", vec![10, 11], vec!["t"])).unwrap();
        let b = create(&mut conn, &pin("Arch", vec![], vec![])).unwrap();
        archive(
            &conn,
            &Project {
                project_fk: Some(b),
            },
        )
        .unwrap();
        // a note on project a to exercise note_counts.
        conn.execute(
            "INSERT INTO NOTE (SOURCE_FK, PROJECT_FK, TITLE, NOTE, CREATED_AT, UPDATED_AT) \
             VALUES (10, ?1, 't', 'c', '2024-01-01T00:00:00', '2024-01-01T00:00:00')",
            [a],
        )
        .unwrap();

        // filter by status.
        let actives = get_many(
            &conn,
            &Projects {
                status: Some(Status::Active),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(actives.len(), 1);
        assert_eq!(actives[0].id, Some(a));
        assert_eq!(actives[0].project_tags, vec!["t"]);

        // filter by explicit ids (both statuses).
        let by_ids = get_many(
            &conn,
            &Projects {
                project_fks: Some(vec![a, b]),
                status: None,
            },
        )
        .unwrap();
        assert_eq!(by_ids.len(), 2);
    }

    #[test]
    fn unset_keys_short_circuit_and_color_helpers() {
        let conn = setup();
        assert!(get(&conn, &Project { project_fk: None }).unwrap().is_none());
        delete(&conn, &Project { project_fk: None }).unwrap();
        restore(&conn, &Project { project_fk: None }).unwrap();
        assert_eq!(color_to_hex(0x00ff00), "#00ff00");
        assert_eq!(color_from_hex("#00ff00").unwrap(), 0x00ff00);
    }

    #[test]
    fn ensure_share_id_idempotent() {
        let mut conn = setup();
        let id = create(&mut conn, &pin("P", vec![], vec![])).unwrap();
        let share_id_1 = ensure_share_id(&conn, id).unwrap();
        let share_id_2 = ensure_share_id(&conn, id).unwrap();
        assert_eq!(share_id_1, share_id_2);
    }

    /// Reverse lookup used by the share layer to route an inbound share_id to a
    /// project. Trashed holders are invisible; archived ones are not.
    #[test]
    fn find_by_share_id_hits_live_misses_unknown_and_trashed() {
        let mut conn = setup();
        let id = create(&mut conn, &pin("P", vec![], vec![])).unwrap();
        let share_id = ensure_share_id(&conn, id).unwrap();

        assert_eq!(find_by_share_id(&conn, &share_id).unwrap(), Some(id));
        assert_eq!(find_by_share_id(&conn, "nobody-holds-this").unwrap(), None);
        assert_eq!(find_by_share_id(&conn, "").unwrap(), None);

        // Archived still resolves — the query only excludes STATUS = 'deleted'.
        archive(
            &conn,
            &Project {
                project_fk: Some(id),
            },
        )
        .unwrap();
        assert_eq!(find_by_share_id(&conn, &share_id).unwrap(), Some(id));

        // Trashed disappears, even though SHARE_ID is still on the row.
        delete(
            &conn,
            &Project {
                project_fk: Some(id),
            },
        )
        .unwrap();
        assert_eq!(find_by_share_id(&conn, &share_id).unwrap(), None);

        // ...and comes back on restore.
        restore(
            &conn,
            &Project {
                project_fk: Some(id),
            },
        )
        .unwrap();
        assert_eq!(find_by_share_id(&conn, &share_id).unwrap(), Some(id));
    }

    #[test]
    fn adopt_share_id_errors_when_claimed_by_live_project() {
        let mut conn = setup();
        let id1 = create(&mut conn, &pin("P1", vec![], vec![])).unwrap();
        let id2 = create(&mut conn, &pin("P2", vec![], vec![])).unwrap();
        let share_id = ensure_share_id(&conn, id1).unwrap();
        adopt_share_id(&conn, id2, &share_id).unwrap_err();
        let share_id_2: Option<String> = conn
            .query_row(
                "SELECT SHARE_ID FROM PROJECT WHERE PROJECT_FK = ?1",
                [id2],
                |row| row.get(0),
            )
            .unwrap();
        assert!(share_id_2.is_none());
    }
}
