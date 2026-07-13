//! Paper service — Rust port of the non-import parts of `service/paper.py`.
//! Plan §5.2. Thin orchestration over `storage::queries::paper` (+ `::project`
//! for project_fks); no raw SQL, no duplicated storage logic.
//!
//! DI seam: every DB-touching fn takes `conn` first; FS-touching helpers
//! (`pdf_on_disk_name`) take only the source_id/version and return a name —
//! they never read config. `import_pdf` lives in `service::paper_import`.
//!
//! Two PDF filename formats stay DISTINCT (do not unify): the on-disk
//! `{safe}v{version}.pdf` here vs. the `.lxproj` archive `{source_id}_v{version}.pdf`
//! (export/import contract). PaperDetails (one version) / PaperDetailsAll (all
//! versions) / SearchResultOut stay distinct serializers (D16).
//!
//! Several Python `storage.db` reads have no dedicated Rust storage fn yet
//! (`get_paper_by_id`, `get_paper_by_source_fk`, `get_categories`,
//! `get_papers_by_json_tag`); they are composed here from existing storage fns
//! rather than adding raw SQL to the service.

use crate::error::Result;
use crate::models::{PaperDetails, PaperDetailsAll, PaperIn, PaperMetadata};
use crate::storage::queries::{paper as store, project as proj_store};
use chrono::{NaiveDate, NaiveDateTime};
use rusqlite::Connection;
use serde::Serialize;

// ── lookup query objects (defined in service/paper.py, not models.py) ────────

/// Identifies a single paper — supply one of the three keys. `version` is only
/// meaningful alongside `source_id` (ignored when paper_id/source_fk drive).
#[derive(Debug, Default, Clone)]
pub struct Paper {
    pub source_fk: Option<i64>,
    pub paper_id: Option<i64>,
    pub source_id: Option<String>,
    pub version: Option<i64>,
}

/// Filter criteria for listing multiple papers.
#[derive(Debug, Default, Clone)]
pub struct Papers {
    pub source_fks: Option<Vec<i64>>,
    pub paper_ids: Option<Vec<i64>>,
    pub source_ids: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
}

/// A soft-deleted paper enriched with its project memberships (Python
/// `DeletedPaperDetails`). Wraps storage's `DeletedPaper` + `project_fks`.
#[derive(Debug, Clone, Serialize)]
pub struct DeletedPaperDetails {
    pub source_fk: i64,
    pub source_id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub published: Option<NaiveDate>,
    pub deleted_at: Option<NaiveDateTime>,
    pub pdf_path: Option<String>,
    pub had_pdf: bool,
    pub project_fks: Vec<i64>,
}

// ── PDF filename helpers (pure; no FS/DB) ────────────────────────────────────

/// `[/\\:*?"<>|]` → `_`, for embedding a source_id in a filename.
const UNSAFE_FNAME: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// Filesystem-safe form of a source_id for PDF filenames.
pub fn pdf_filename_safe(source_id: &str) -> String {
    source_id
        .chars()
        .map(|c| if UNSAFE_FNAME.contains(&c) { '_' } else { c })
        .collect()
}

/// On-disk name for a directly-imported PDF: `{safe}v{version}.pdf` (NO
/// underscore before `v`). Distinct from the `.lxproj` archive format
/// `{source_id}_v{version}.pdf` — do not unify.
pub fn pdf_on_disk_name(source_id: &str, version: i64) -> String {
    format!("{}v{}.pdf", pdf_filename_safe(source_id), version)
}

// ── composed reads (no dedicated storage fn exists yet) ──────────────────────

/// `db.get_paper_by_id` — exact PAPER version by PK. Composed by scanning the
/// all-versions list (same `papers` view the Python query hits).
fn paper_by_id(conn: &Connection, paper_id: i64) -> Result<Option<PaperDetails>> {
    Ok(store::list_papers(conn, false, None, 0, None)?
        .into_iter()
        .find(|p| p.paper_id == paper_id))
}

/// `db.get_paper_by_source_fk` — latest version for a root. Composed:
/// SOURCE_FK → SOURCE_ID → latest paper.
fn paper_by_source_fk(conn: &Connection, source_fk: i64) -> Result<Option<PaperDetails>> {
    match store::get_source_id(conn, source_fk)? {
        Some(sid) => store::get_paper(conn, &sid, None),
        None => Ok(None),
    }
}

// ── lookup seam ──────────────────────────────────────────────────────────────

/// Fetch a single paper version. Resolution order:
/// `paper_id` → `source_fk` → `source_id` (`version` only applies to source_id).
pub fn get(conn: &Connection, paper: &Paper) -> Result<Option<PaperDetails>> {
    if let Some(pid) = paper.paper_id {
        return paper_by_id(conn, pid); // version (if any) ignored
    }
    if let Some(sfk) = paper.source_fk {
        return paper_by_source_fk(conn, sfk); // version ignored
    }
    if let Some(sid) = paper.source_id.as_deref() {
        return store::get_paper(conn, sid, paper.version);
    }
    Ok(None)
}

/// Fetch every stored version, display fields from the latest. Resolution order
/// DIFFERS from `get`: `source_id` → `paper_id` → `source_fk`.
pub fn get_all(conn: &Connection, paper: &Paper) -> Result<Option<PaperDetailsAll>> {
    let source_id = if let Some(sid) = paper.source_id.clone() {
        sid
    } else if let Some(pid) = paper.paper_id {
        match paper_by_id(conn, pid)? {
            Some(p) => p.source_id,
            None => return Ok(None),
        }
    } else if let Some(sfk) = paper.source_fk {
        match paper_by_source_fk(conn, sfk)? {
            Some(p) => p.source_id,
            None => return Ok(None),
        }
    } else {
        return Ok(None);
    };

    let rows = store::get_all_versions(conn, &source_id)?;
    if rows.is_empty() {
        return Ok(None);
    }
    let latest = rows.last().expect("non-empty");
    Ok(Some(PaperDetailsAll {
        source_id: source_id.clone(),
        latest_version: latest.version,
        title: latest.title.clone(),
        authors: latest.authors.clone(),
        summary: latest.summary.clone(),
        published: rows[0].published, // oldest version's published date
        updated: latest.updated,
        doi: latest.doi.clone(),
        url: latest.url.clone(),
        category: latest.category.clone(),
        categories: latest.categories.clone(),
        journal_ref: latest.journal_ref.clone(),
        comment: latest.comment.clone(),
        tags: latest.tags.clone(),
        source: latest.source.clone(),
        versions: rows,
    }))
}

/// Fetch multiple papers matching any combination of `Papers` filters
/// (latest version per paper). Empty filter lists are no-ops (Python truthiness).
pub fn get_many(conn: &Connection, papers: &Papers) -> Result<Vec<PaperDetails>> {
    let mut results = store::list_papers(conn, true, None, 0, None)?;

    results.retain(|row| {
        if let Some(ids) = &papers.paper_ids {
            if !ids.is_empty() && !ids.contains(&row.paper_id) {
                return false;
            }
        }
        if let Some(sids) = &papers.source_ids {
            if !sids.is_empty() && !sids.contains(&row.source_id) {
                return false;
            }
        }
        if let Some(tags) = &papers.tags {
            if !tags.is_empty() && !tags.iter().any(|t| row.tags.contains(t)) {
                return false;
            }
        }
        true
    });

    if let Some(fks) = &papers.source_fks {
        if !fks.is_empty() {
            let mut kept = Vec::new();
            for detail in results {
                if let Some(root) = store::get_paper_root(conn, &detail.source_id)? {
                    if fks.contains(&root.source_fk) {
                        kept.push(detail);
                    }
                }
            }
            results = kept;
        }
    }
    Ok(results)
}

// ── writes ───────────────────────────────────────────────────────────────────

/// Insert/update from the GUI `PaperIn` DTO. Returns (source_id, version).
pub fn upsert(
    conn: &mut Connection,
    paper: &PaperIn,
    tags: Option<&[String]>,
) -> Result<(String, i64)> {
    let meta = PaperMetadata {
        source_id: paper.source_id.clone().unwrap_or_default(),
        // Python `paper.version or 1`: 0 is falsy → 1.
        version: paper.version.filter(|v| *v != 0).unwrap_or(1),
        title: paper.title.clone(),
        authors: paper.authors.clone().unwrap_or_default(),
        published: paper.published,
        updated: None,
        summary: paper.summary.clone().unwrap_or_default(),
        category: paper.category.clone(),
        categories: None,
        doi: paper.doi.clone(),
        journal_ref: None,
        comment: None,
        url: paper.url.clone(),
        tags: paper.tags.clone(),
        source: paper.source.clone(),
    };
    store::save_paper_metadata(conn, &meta, tags)
}

/// Persist one normalized record. Returns (source_id, version).
pub fn save_paper_metadata(
    conn: &mut Connection,
    meta: &PaperMetadata,
    tags: Option<&[String]>,
) -> Result<(String, i64)> {
    store::save_paper_metadata(conn, meta, tags)
}

/// UNION `tags` onto a paper's existing tags (dual JSON + relational storage).
/// Import/share merge path. Returns the merged list.
pub fn add_paper_tags(
    conn: &mut Connection,
    source_id: &str,
    tags: &[String],
) -> Result<Vec<String>> {
    store::add_paper_tags(conn, source_id, tags)
}

/// Re-write a paper's metadata in-place (migrating SOURCE_ID if it changed).
pub fn repair_paper(conn: &mut Connection, source_fk: i64, meta: &PaperMetadata) -> Result<()> {
    store::repair_paper(conn, source_fk, meta)
}

// ── soft / hard delete ───────────────────────────────────────────────────────

/// Resolve a `Paper` to its text source_id. Order: source_id → source_fk → paper_id.
fn resolve_source_id(conn: &Connection, paper: &Paper) -> Result<Option<String>> {
    if let Some(sid) = paper.source_id.clone() {
        return Ok(Some(sid));
    }
    if let Some(sfk) = paper.source_fk {
        return store::get_source_id(conn, sfk);
    }
    if let Some(pid) = paper.paper_id {
        return Ok(paper_by_id(conn, pid)?.map(|p| p.source_id));
    }
    Ok(None)
}

/// Soft-delete a paper. Returns its stored PDF_PATH (for the caller to unlink).
pub fn delete(conn: &mut Connection, paper: &Paper) -> Result<Option<String>> {
    match resolve_source_id(conn, paper)? {
        Some(sid) => store::soft_delete_paper(conn, &sid),
        None => Ok(None),
    }
}

/// Restore a soft-deleted paper. Returns (pdf_path, project_fks). The root's
/// SOURCE_FK is read before the restore write — it's an immutable PK, no TOCTOU.
pub fn restore(conn: &mut Connection, paper: &Paper) -> Result<(Option<String>, Vec<i64>)> {
    let Some(source_id) = resolve_source_id(conn, paper)? else {
        return Ok((None, Vec::new()));
    };
    let root = store::get_paper_root(conn, &source_id)?;
    let pdf_path = store::restore_paper(conn, &source_id)?;
    let project_fks = match root {
        Some(r) => proj_store::get_paper_project_fks(conn, r.source_fk)?,
        None => Vec::new(),
    };
    Ok((pdf_path, project_fks))
}

/// Permanently remove a paper (children cascade off the FK).
pub fn hard_delete(conn: &mut Connection, paper: &Paper) -> Result<Option<String>> {
    match resolve_source_id(conn, paper)? {
        Some(sid) => store::hard_delete_paper(conn, &sid),
        None => Ok(None),
    }
}

/// All soft-deleted papers, each enriched with its project memberships.
pub fn list_deleted(conn: &Connection) -> Result<Vec<DeletedPaperDetails>> {
    let mut out = Vec::new();
    for row in store::list_deleted_papers(conn)? {
        let project_fks = proj_store::get_paper_project_fks(conn, row.source_fk)?;
        out.push(DeletedPaperDetails {
            source_fk: row.source_fk,
            source_id: row.source_id,
            title: row.title,
            authors: row.authors,
            published: row.published,
            deleted_at: row.deleted_at,
            pdf_path: row.pdf_path,
            had_pdf: row.had_pdf,
            project_fks,
        });
    }
    Ok(out)
}

/// True if the paper exists in soft-deleted state.
pub fn is_paper_deleted(conn: &Connection, source_id: &str) -> Result<bool> {
    store::is_paper_deleted(conn, source_id)
}

// ── multi-paper reads ────────────────────────────────────────────────────────

/// Latest-version (default) paper rows, with optional exact-category filter and
/// limit/offset. In Rust storage already returns `PaperDetails`, so this is the
/// merge of Python's `list_papers` + `list_paper_details`.
pub fn list_papers(
    conn: &Connection,
    latest_only: bool,
    limit: Option<i64>,
    offset: i64,
    category: Option<&str>,
) -> Result<Vec<PaperDetails>> {
    store::list_papers(conn, latest_only, limit, offset, category)
}

/// Sorted distinct primary categories across latest papers (`db.get_categories`).
pub fn get_categories(conn: &Connection) -> Result<Vec<String>> {
    let set: std::collections::BTreeSet<String> = store::list_papers(conn, true, None, 0, None)?
        .into_iter()
        .filter_map(|p| p.category)
        .collect();
    Ok(set.into_iter().collect())
}

/// Latest papers whose JSON tags include `label`, case-insensitively
/// (`db.get_papers_by_json_tag`). Order: published DESC, then paper_id DESC so
/// same-published-date papers are deterministic (matches the SQL secondary sort).
pub fn get_papers_by_tag(conn: &Connection, label: &str) -> Result<Vec<PaperDetails>> {
    let mut out: Vec<PaperDetails> = store::list_papers(conn, true, None, 0, None)?
        .into_iter()
        .filter(|p| p.tags.iter().any(|t| t.eq_ignore_ascii_case(label)))
        .collect();
    out.sort_by(|a, b| {
        b.published
            .cmp(&a.published)
            .then(b.paper_id.cmp(&a.paper_id))
    });
    Ok(out)
}

// ── root / id helpers ────────────────────────────────────────────────────────

/// Insert PAPER_ROOTS row if absent (reactivating if soft-deleted). Returns SOURCE_FK.
pub fn ensure_paper_root(conn: &mut Connection, source_id: &str) -> Result<i64> {
    store::ensure_paper_root(conn, source_id)
}

/// SOURCE_ID for a SOURCE_FK, or None.
pub fn get_source_id(conn: &Connection, source_fk: i64) -> Result<Option<String>> {
    store::get_source_id(conn, source_fk)
}

/// Resolve a list of SOURCE_FKs to SOURCE_IDs, dropping any that don't exist.
pub fn sfks_to_source_ids(conn: &Connection, source_fks: &[i64]) -> Result<Vec<String>> {
    store::sfks_to_source_ids(conn, source_fks)
}

// ── PDF / full-text setters ──────────────────────────────────────────────────

/// Set HAS_PDF for one version.
pub fn set_has_pdf(conn: &Connection, source_id: &str, version: i64, has: bool) -> Result<()> {
    store::set_has_pdf(conn, source_id, version, has)
}

/// Set PDF_PATH for one version, or every version when `version` is None/0.
pub fn set_pdf_path(
    conn: &Connection,
    source_id: &str,
    path: &str,
    version: Option<i64>,
) -> Result<()> {
    store::set_pdf_path(conn, source_id, path, version)
}

/// Atomically record PDF_PATH and set HAS_PDF=1 for one version.
pub fn mark_pdf_saved(
    conn: &mut Connection,
    source_id: &str,
    path: &str,
    version: i64,
) -> Result<()> {
    store::mark_pdf_saved(conn, source_id, path, version)
}

/// Store extracted full text + refresh the FTS index for one version.
pub fn set_full_text(
    conn: &mut Connection,
    source_id: &str,
    version: i64,
    full_text: &str,
) -> Result<()> {
    store::set_full_text(conn, source_id, version, Some(full_text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{db::open_in_memory, init_db};

    fn meta(source_id: &str, version: i64, category: &str, tags: &[&str]) -> PaperMetadata {
        PaperMetadata {
            source_id: source_id.into(),
            version,
            title: format!("T{version}"),
            authors: vec!["Alice".into(), "Bob".into()],
            published: NaiveDate::from_ymd_opt(2024, 1, version as u32).unwrap(),
            updated: None,
            summary: "sum".into(),
            category: Some(category.into()),
            categories: Some(vec![category.into()]),
            doi: Some("10.1/x".into()),
            journal_ref: None,
            comment: None,
            url: Some("http://x".into()),
            tags: Some(tags.iter().map(|t| t.to_string()).collect()),
            source: Some("arxiv".into()),
        }
    }

    fn mem() -> Connection {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn
    }

    // Two versions of one paper. Returns (source_fk, paper_id_v1, paper_id_v2).
    fn seed_two_versions(conn: &mut Connection) -> (i64, i64, i64) {
        save_paper_metadata(conn, &meta("arxiv:A", 1, "cs.LG", &["ml"]), None).unwrap();
        save_paper_metadata(conn, &meta("arxiv:A", 2, "cs.LG", &["ml"]), None).unwrap();
        let fk = ensure_paper_root(conn, "arxiv:A").unwrap();
        let v1 = get(
            conn,
            &Paper {
                source_id: Some("arxiv:A".into()),
                version: Some(1),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap()
        .paper_id;
        let v2 = get(
            conn,
            &Paper {
                source_id: Some("arxiv:A".into()),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap()
        .paper_id;
        (fk, v1, v2)
    }

    #[test]
    fn get_dispatch_priority_and_version_handling() {
        let mut conn = mem();
        let (fk, v1, v2) = seed_two_versions(&mut conn);

        // source_fk -> latest version.
        assert_eq!(
            get(
                &conn,
                &Paper {
                    source_fk: Some(fk),
                    ..Default::default()
                }
            )
            .unwrap()
            .unwrap()
            .version,
            2
        );
        // paper_id -> that exact version.
        assert_eq!(
            get(
                &conn,
                &Paper {
                    paper_id: Some(v1),
                    ..Default::default()
                }
            )
            .unwrap()
            .unwrap()
            .version,
            1
        );
        // source_id + version -> pinned; source_id alone -> latest.
        assert_eq!(
            get(
                &conn,
                &Paper {
                    source_id: Some("arxiv:A".into()),
                    version: Some(1),
                    ..Default::default()
                }
            )
            .unwrap()
            .unwrap()
            .version,
            1
        );
        assert_eq!(
            get(
                &conn,
                &Paper {
                    source_id: Some("arxiv:A".into()),
                    ..Default::default()
                }
            )
            .unwrap()
            .unwrap()
            .version,
            2
        );

        // Priority paper_id > source_fk > source_id; version ignored for paper_id.
        let p = get(
            &conn,
            &Paper {
                paper_id: Some(v2),
                source_fk: Some(999),
                source_id: Some("nope".into()),
                version: Some(1),
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(p.version, 2);
        // source_fk beats source_id.
        let p = get(
            &conn,
            &Paper {
                source_fk: Some(fk),
                source_id: Some("nope".into()),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(p.version, 2);

        assert!(get(&conn, &Paper::default()).unwrap().is_none());
        let _ = v1;
    }

    #[test]
    fn get_all_aggregates_across_versions_each_dispatch() {
        let mut conn = mem();
        let (fk, v1, _v2) = seed_two_versions(&mut conn);

        for key in [
            Paper {
                source_id: Some("arxiv:A".into()),
                ..Default::default()
            },
            Paper {
                paper_id: Some(v1),
                ..Default::default()
            },
            Paper {
                source_fk: Some(fk),
                ..Default::default()
            },
        ] {
            let all = get_all(&conn, &key).unwrap().unwrap();
            assert_eq!(all.source_id, "arxiv:A");
            assert_eq!(all.latest_version, 2);
            assert_eq!(all.title, "T2"); // display from latest
            assert_eq!(
                all.versions.iter().map(|v| v.version).collect::<Vec<_>>(),
                vec![1, 2]
            );
            // published comes from the OLDEST version (rows[0]).
            assert_eq!(all.published, NaiveDate::from_ymd_opt(2024, 1, 1));
        }

        assert!(get_all(
            &conn,
            &Paper {
                paper_id: Some(424242),
                ..Default::default()
            }
        )
        .unwrap()
        .is_none());
        assert!(get_all(&conn, &Paper::default()).unwrap().is_none());
    }

    #[test]
    fn get_many_filters() {
        let mut conn = mem();
        save_paper_metadata(&mut conn, &meta("arxiv:A", 1, "cs.LG", &["ml"]), None).unwrap();
        save_paper_metadata(&mut conn, &meta("arxiv:B", 1, "math.CO", &["theory"]), None).unwrap();
        let fk_a = ensure_paper_root(&mut conn, "arxiv:A").unwrap();
        let pid_b = get(
            &conn,
            &Paper {
                source_id: Some("arxiv:B".into()),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap()
        .paper_id;

        // No filters -> both latest papers.
        assert_eq!(get_many(&conn, &Papers::default()).unwrap().len(), 2);
        // Empty lists are no-ops, not "match nothing".
        assert_eq!(
            get_many(
                &conn,
                &Papers {
                    paper_ids: Some(vec![]),
                    source_ids: Some(vec![]),
                    tags: Some(vec![]),
                    source_fks: Some(vec![]),
                    ..Default::default()
                }
            )
            .unwrap()
            .len(),
            2
        );
        // source_ids filter.
        let r = get_many(
            &conn,
            &Papers {
                source_ids: Some(vec!["arxiv:B".into()]),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].source_id, "arxiv:B");
        // tags filter (any-match).
        let r = get_many(
            &conn,
            &Papers {
                tags: Some(vec!["ml".into()]),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].source_id, "arxiv:A");
        // paper_ids filter.
        let r = get_many(
            &conn,
            &Papers {
                paper_ids: Some(vec![pid_b]),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].source_id, "arxiv:B");
        // source_fks filter (resolved via paper root).
        let r = get_many(
            &conn,
            &Papers {
                source_fks: Some(vec![fk_a]),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].source_id, "arxiv:A");
    }

    #[test]
    fn upsert_roundtrip_and_categories_and_by_tag() {
        let mut conn = mem();
        let p = PaperIn {
            title: "Manual".into(),
            published: NaiveDate::from_ymd_opt(2024, 5, 1).unwrap(),
            source_id: Some("local:abc".into()),
            version: None,
            authors: Some(vec!["Carol".into()]),
            summary: Some("s".into()),
            category: Some("cs.AI".into()),
            doi: None,
            url: None,
            tags: Some(vec!["Mine".into()]),
            source: Some("local".into()),
        };
        let (sid, ver) = upsert(&mut conn, &p, Some(&["shared".into()])).unwrap();
        assert_eq!((sid.as_str(), ver), ("local:abc", 1)); // version None/0 -> 1

        let got = get(
            &conn,
            &Paper {
                source_id: Some("local:abc".into()),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(got.title, "Manual");
        assert_eq!(got.authors, vec!["Carol".to_string()]);
        // upsert tags + extra tags merged.
        assert!(got.tags.contains(&"Mine".to_string()) && got.tags.contains(&"shared".to_string()));

        save_paper_metadata(&mut conn, &meta("arxiv:Z", 1, "cs.LG", &["other"]), None).unwrap();
        // categories: sorted distinct.
        assert_eq!(
            get_categories(&conn).unwrap(),
            vec!["cs.AI".to_string(), "cs.LG".to_string()]
        );
        // by_tag case-insensitive.
        let hits = get_papers_by_tag(&conn, "mine").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source_id, "local:abc");
        assert!(get_papers_by_tag(&conn, "nope").unwrap().is_empty());
    }

    #[test]
    fn get_papers_by_tag_tiebreaks_same_date_by_paper_id_desc() {
        let mut conn = mem();
        // Two papers share the same published date (version 1 -> 2024-01-01) and tag.
        save_paper_metadata(&mut conn, &meta("arxiv:A", 1, "cs.LG", &["shared"]), None).unwrap();
        save_paper_metadata(&mut conn, &meta("arxiv:B", 1, "cs.LG", &["shared"]), None).unwrap();

        let hits = get_papers_by_tag(&conn, "shared").unwrap();
        assert_eq!(hits.len(), 2);
        // Same published date -> deterministic paper_id DESC (B saved after A).
        assert!(
            hits[0].paper_id > hits[1].paper_id,
            "same-date papers ordered by paper_id DESC"
        );
    }

    #[test]
    fn delete_restore_hard_delete_resolve_via_keys() {
        let mut conn = mem();
        let (fk, v1, _v2) = seed_two_versions(&mut conn);
        // Link to a project so restore returns its membership.
        conn.execute(
            "INSERT INTO PROJECT (NAME, STATUS) VALUES ('P', 'active')",
            [],
        )
        .unwrap();
        let proj = conn.last_insert_rowid();
        proj_store::add_papers(&conn, proj, &[fk]).unwrap();

        // Soft-delete resolved via paper_id.
        delete(
            &mut conn,
            &Paper {
                paper_id: Some(v1),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(is_paper_deleted(&conn, "arxiv:A").unwrap());
        let deleted = list_deleted(&conn).unwrap();
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0].source_id, "arxiv:A");
        assert_eq!(deleted[0].project_fks, vec![proj]); // enriched

        // Restore resolved via source_fk -> returns project_fks.
        let (_pdf, fks) = restore(
            &mut conn,
            &Paper {
                source_fk: Some(fk),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(fks, vec![proj]);
        assert!(!is_paper_deleted(&conn, "arxiv:A").unwrap());

        // Hard-delete resolved via source_id.
        hard_delete(
            &mut conn,
            &Paper {
                source_id: Some("arxiv:A".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(get(
            &conn,
            &Paper {
                source_id: Some("arxiv:A".into()),
                ..Default::default()
            }
        )
        .unwrap()
        .is_none());

        // No-key ops are no-ops.
        assert!(delete(&mut conn, &Paper::default()).unwrap().is_none());
    }

    #[test]
    fn pdf_and_fulltext_setters() {
        let mut conn = mem();
        save_paper_metadata(&mut conn, &meta("arxiv:p", 1, "cs.LG", &[]), None).unwrap();

        set_has_pdf(&conn, "arxiv:p", 1, true).unwrap();
        assert!(
            get(
                &conn,
                &Paper {
                    source_id: Some("arxiv:p".into()),
                    version: Some(1),
                    ..Default::default()
                }
            )
            .unwrap()
            .unwrap()
            .has_pdf
        );

        set_pdf_path(&conn, "arxiv:p", "/tmp/a.pdf", Some(1)).unwrap();
        mark_pdf_saved(&mut conn, "arxiv:p", "/tmp/b.pdf", 1).unwrap();
        let got = get(
            &conn,
            &Paper {
                source_id: Some("arxiv:p".into()),
                version: Some(1),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(got.pdf_path.as_deref(), Some("/tmp/b.pdf"));
        assert!(got.has_pdf);

        set_full_text(&mut conn, "arxiv:p", 1, "the full tex body").unwrap();
        let got = get(
            &conn,
            &Paper {
                source_id: Some("arxiv:p".into()),
                version: Some(1),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(got.full_text.as_deref(), Some("the full tex body"));
        assert!(got.downloaded_source);
    }

    #[test]
    fn root_and_id_helpers() {
        let mut conn = mem();
        save_paper_metadata(&mut conn, &meta("arxiv:v", 1, "cs.LG", &[]), None).unwrap();
        let fk = ensure_paper_root(&mut conn, "arxiv:v").unwrap();
        assert_eq!(
            get_source_id(&conn, fk).unwrap().as_deref(),
            Some("arxiv:v")
        );
        assert_eq!(
            sfks_to_source_ids(&conn, &[fk, 9_999]).unwrap(),
            vec!["arxiv:v".to_string()]
        );
    }

    #[test]
    fn pure_filename_and_parse_helpers() {
        assert_eq!(pdf_filename_safe("arxiv:2204.12985"), "arxiv_2204.12985");
        assert_eq!(
            pdf_filename_safe(r#"a/b\c:d*e?f"g<h>i|j"#),
            "a_b_c_d_e_f_g_h_i_j"
        );
        // On-disk format: no underscore before 'v'.
        assert_eq!(
            pdf_on_disk_name("arxiv:2204.12985", 4),
            "arxiv_2204.12985v4.pdf"
        );
    }
}
