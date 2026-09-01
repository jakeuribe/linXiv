//! `/api/papers` routes — `api/app.py` 204–261, 365–379, 433–461, 1135–1139.
//! Mirrors the `papers` MCP cluster (`mcp/src/papers.rs`) over the same core
//! service (`service::paper`). Shape copied from `route/authors.rs`.
//!
//! The generic `{source_id}` arms match EXACTLY 3 segments; the `/pdf` and
//! `/pdf-path` subtrees belong to the `pdfs` group (tried first in `mod.rs`).
//! `POST {source_id}/full-text` is the one 4-segment arm this group owns.

use serde::Deserialize;
use serde_json::{json, Value};

use linxiv_core::config;
use linxiv_core::error::CoreError;
use linxiv_core::service::paper::{self as svc_paper, PaperRef};
use linxiv_core::service::paper_merge as svc_merge;
use linxiv_core::service::project as svc_project;

use crate::route::{path_i64, ApiError, ReqCtx};
use crate::state::AppState;

/// Returns `Some(result)` if this group owns `(method, path)`, else `None`.
pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("GET", ["api", "papers"]) => Some(list(state, ctx)),
        ("GET", ["api", "papers", "sfk", fk, "versions"]) => Some(versions(state, fk)),
        ("GET", ["api", "papers", "sfk", fk, "doi-candidates"]) => Some(doi_candidates(state, fk)),
        ("GET", ["api", "papers", "sfk", fk]) => Some(by_sfk(state, fk, ctx)),
        ("PUT", ["api", "papers", "sfk", fk]) => Some(repair(state, fk, ctx)),
        ("POST", ["api", "papers", "sfk", fk, "merge"]) => Some(merge(state, fk, ctx)),
        ("DELETE", ["api", "papers", "sfk", fk, "projects"]) => {
            Some(remove_from_projects(state, fk))
        }
        // `search` and `full-text-pending` must precede the generic
        // `{source_id}` arm (all 3 segments).
        ("GET", ["api", "papers", "search"]) => Some(search(state, ctx)),
        ("GET", ["api", "papers", "full-text-pending"]) => Some(full_text_pending(state)),
        ("POST", ["api", "papers", id, "full-text"]) => Some(fetch_full_text(state, id, ctx).await),
        ("GET", ["api", "papers", id]) => Some(get_one(state, id)),
        ("DELETE", ["api", "papers", id]) => Some(delete(state, id)),
        _ => None,
    }
}

/// `GET /api/papers?limit=&offset=&sort=&dir=` — `api_list_papers`.
/// `sort` is one of `published` (default) / `added` / `title`; `dir` is
/// `asc`/`desc`, defaulting per metric (newest first, titles A–Z).
fn list(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let limit = ctx.q_i64("limit").unwrap_or(200).clamp(1, 5000);
    let offset = ctx.q_i64("offset").unwrap_or(0).max(0);
    let sort = svc_paper::PaperSort::from_key(ctx.q("sort").unwrap_or_default());
    let desc = match ctx.q("dir") {
        Some("asc") => false,
        Some("desc") => true,
        _ => sort.default_desc(),
    };
    let papers = state.with_conn(|conn| {
        svc_paper::list_papers_sorted(conn, true, Some(limit), offset, None, sort, desc)
    })?;
    Ok(json!({ "papers": papers }))
}

/// `GET /api/papers/sfk/{fk}/versions` — `api_get_paper_versions`.
fn versions(state: &AppState, fk: &str) -> Result<Value, ApiError> {
    let source_fk = path_i64(fk)?;
    let all = state.with_conn(|conn| svc_paper::get_all(conn, &sfk_key(source_fk)))?;
    let all = all.ok_or(CoreError::PaperNotFound(source_fk.to_string()))?;
    let versions: Vec<Value> = all
        .versions
        .iter()
        .map(|v| {
            json!({
                "version": v.version,
                "published": v.published, // Option<NaiveDate> -> ISO string or null
                "updated": v.updated,
                "has_pdf": v.has_pdf,
            })
        })
        .collect();
    Ok(json!({
        "source_id": all.source_id,
        "latest_version": all.latest_version,
        "versions": versions,
    }))
}

/// `GET /api/papers/sfk/{fk}/doi-candidates` — other paper roots sharing this
/// one's DOI, for the "same paper, different source" suggestion banner.
fn doi_candidates(state: &AppState, fk: &str) -> Result<Value, ApiError> {
    let source_fk = path_i64(fk)?;
    let candidates = state.with_conn(|conn| -> Result<_, ApiError> {
        if svc_paper::get_source_id(conn, source_fk)?.is_none() {
            return Err(CoreError::PaperNotFound(source_fk.to_string()).into());
        }
        Ok(svc_paper::find_doi_version_candidates(conn, source_fk)?)
    })?;
    Ok(json!({ "candidates": candidates }))
}

/// `GET /api/papers/sfk/{fk}?version=` — `api_get_paper_by_sfk`. Bare `to_dict()`.
fn by_sfk(state: &AppState, fk: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let source_fk = path_i64(fk)?;
    // FastAPI Query(default=None, ge=1): a present-but-non-integer or <1 version is
    // a 422, not a silent fall-through to the latest version.
    let version = crate::route::q_version(ctx)?;
    state.with_conn(|conn| -> Result<Value, ApiError> {
        let paper = if let Some(version) = version {
            // version branch: resolve source_id first, then the pinned version.
            let source_id = svc_paper::get_source_id(conn, source_fk)?
                .ok_or_else(|| CoreError::PaperNotFound(source_fk.to_string()))?;
            let key = PaperRef::Source {
                source_id,
                version: Some(version),
            };
            svc_paper::get(conn, &key)?
                .ok_or_else(|| ApiError::new(404, format!("Version {version} not stored")))?
        } else {
            svc_paper::get(conn, &sfk_key(source_fk))?
                .ok_or_else(|| CoreError::PaperNotFound(source_fk.to_string()))?
        };
        to_value(&paper)
    })
}

/// `GET /api/papers/search?q=&limit=` — `api_search_papers`.
fn search(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let q = ctx.q("q").unwrap_or("").trim().to_string();
    if q.chars().count() < 3 {
        return Err(ApiError::new(
            422,
            "Query must contain at least 3 non-whitespace characters",
        ));
    }
    let limit = ctx.q_i64("limit").unwrap_or(50).clamp(1, 100);
    let papers = state.with_conn(|conn| svc_paper::search_library(conn, &q, limit))?;
    Ok(json!({ "papers": papers }))
}

/// `POST /api/papers/{source_id}/full-text?force=` — the write half of `search`
/// above. Downloads the paper's arXiv TeX tarball, extracts it, and stores the
/// text so `papers_fts` actually has something to match; without this the index
/// only ever held rows carried over by the Python-era migration.
///
/// Not automatic on save: arXiv paces requests ~7s apart and a tarball runs to
/// megabytes, which is too much to spend on every paper the user stores without
/// being asked. The opt-in automatic path is `full_text_worker`, which chews
/// through the same backlog in the background while its setting is on.
async fn fetch_full_text(
    state: &AppState,
    source_id: &str,
    ctx: &ReqCtx<'_>,
) -> Result<Value, ApiError> {
    let paper = state
        .with_conn(|conn| svc_paper::get(conn, &sid_key(source_id)))?
        .ok_or_else(|| CoreError::PaperNotFound(source_id.to_string()))?;
    if paper.downloaded_source && !ctx.q_bool("force") {
        return to_value(&svc_paper::FullTextReceipt::already_indexed(&paper));
    }
    let receipt = ingest_full_text(state, &paper).await?;
    to_value(&receipt)
}

/// `GET /api/papers/full-text-pending` — how many stored arXiv papers have no
/// TeX source yet, i.e. how much work `full_text_worker` still has.
fn full_text_pending(state: &AppState) -> Result<Value, ApiError> {
    let pending = state.with_conn(|conn| svc_paper::full_text_backfill_count(conn))?;
    Ok(json!({ "pending": pending }))
}

/// Download + extract + store one paper's TeX (`service::paper`'s two-phase
/// ingest: fetch outside the lock, commit under it). Shared by the route above
/// and `full_text_worker`.
pub(crate) async fn ingest_full_text(
    state: &AppState,
    paper: &linxiv_core::models::PaperDetails,
) -> Result<svc_paper::FullTextReceipt, ApiError> {
    let fetched = svc_paper::fetch_full_text(paper, &config::data_dir()).await?;
    Ok(state.with_conn(|conn| fetched.commit(conn))?)
}

/// `GET /api/papers/{source_id}` — `api_get_paper`. Bare `to_dict()`.
fn get_one(state: &AppState, source_id: &str) -> Result<Value, ApiError> {
    let paper = state.with_conn(|conn| svc_paper::get(conn, &sid_key(source_id)))?;
    let paper = paper.ok_or_else(|| CoreError::PaperNotFound(source_id.to_string()))?;
    to_value(&paper)
}

/// `DELETE /api/papers/{source_id}` — `api_delete_paper`.
fn delete(state: &AppState, source_id: &str) -> Result<Value, ApiError> {
    state.with_conn(|conn| -> Result<(), ApiError> {
        if svc_paper::get(conn, &sid_key(source_id))?.is_none() {
            return Err(CoreError::PaperNotFound(source_id.to_string()).into());
        }
        svc_paper::delete(conn, &sid_key(source_id))?;
        Ok(())
    })?;
    Ok(json!({ "deleted": source_id }))
}

/// `PUT /api/papers/sfk/{fk}` — `api_repair_paper`. Rebuilds metadata from the
/// existing paper's identity (source_id/version/source) + the PUT body.
fn repair(state: &AppState, fk: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let source_fk = path_i64(fk)?;
    let b: svc_paper::RepairFields = ctx.parse_body()?;
    state.with_conn(|conn| -> Result<Value, ApiError> {
        let paper = svc_paper::get(conn, &sfk_key(source_fk))?
            .ok_or_else(|| CoreError::PaperNotFound(source_fk.to_string()))?;
        // Date validated after the existence check, matching MCP and Python.
        let meta = b.into_metadata(paper.source_id, paper.version, paper.source)?;
        // Python maps sqlite3.IntegrityError -> 409, but this endpoint
        // never renames source_id so no UNIQUE conflict can arise; a stray rusqlite
        // error surfaces as CoreError::Internal (500). Reachable paths stay faithful.
        svc_paper::repair_paper(conn, source_fk, &meta)?;
        let updated = svc_paper::get(conn, &sfk_key(source_fk))?
            .ok_or_else(|| ApiError::new(500, "Repair failed"))?;
        to_value(&updated)
    })
}

/// `POST /api/papers/sfk/{fk}/merge` — `merge_papers`. Merges the duplicate
/// root named in the body INTO this paper (this paper's metadata is canonical;
/// the duplicate's notes, annotations, memberships, tags, missing versions and
/// PDFs move over, then the duplicate root is deleted). 404 on unknown roots,
/// 409 on self/trashed/share-linked duplicates (see `merge_plan`'s guards).
fn merge(state: &AppState, fk: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let winner_fk = path_i64(fk)?;
    #[derive(Deserialize)]
    struct Body {
        loser_source_fk: i64,
    }
    let b: Body = ctx.parse_body()?;
    let pdf_dir = state.pdf_dir.clone();
    // Holds the conn lock across the FS phase too — same-dir renames, bounded
    // by the loser's version count.
    let receipt = state.with_conn(|conn| {
        svc_merge::merge_papers(
            conn,
            &pdf_dir,
            &PaperRef::SourceFk(winner_fk),
            &PaperRef::SourceFk(b.loser_source_fk),
        )
    })?;
    to_value(&receipt)
}

/// `DELETE /api/papers/sfk/{fk}/projects` — `api_remove_paper_from_all_projects`.
fn remove_from_projects(state: &AppState, fk: &str) -> Result<Value, ApiError> {
    let source_fk = path_i64(fk)?;
    let removed =
        state.with_conn(|conn| svc_project::remove_paper_from_all_projects(conn, source_fk))?;
    Ok(json!({ "ok": true, "removed_from_projects": removed }))
}

fn sfk_key(source_fk: i64) -> PaperRef {
    PaperRef::SourceFk(source_fk)
}

pub(crate) fn sid_key(source_id: &str) -> PaperRef {
    PaperRef::source(source_id.to_string())
}

use super::to_value;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::{route, ApiRequest};
    use linxiv_core::models::PaperMetadata;
    use linxiv_core::storage;

    fn state() -> AppState {
        let conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        AppState::from_parts(conn, std::env::temp_dir(), std::env::temp_dir())
    }

    async fn req(
        st: &AppState,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, ApiError> {
        route(
            st,
            ApiRequest {
                method: method.into(),
                path: path.into(),
                body,
            },
        )
        .await
    }

    #[tokio::test]
    async fn list_on_empty_db_wraps_empty_array() {
        assert_eq!(
            req(&state(), "GET", "/api/papers", None).await.unwrap(),
            json!({ "papers": [] })
        );
    }

    #[tokio::test]
    async fn get_missing_paper_is_404() {
        let err = req(&state(), "GET", "/api/papers/arxiv:nope", None)
            .await
            .unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Paper arxiv:nope not found");
    }

    #[tokio::test]
    async fn delete_missing_paper_is_404() {
        let err = req(&state(), "DELETE", "/api/papers/arxiv:nope", None)
            .await
            .unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Paper arxiv:nope not found");
    }

    fn meta(source_id: &str, doi: Option<&str>) -> PaperMetadata {
        let mut m: PaperMetadata = serde_json::from_value(json!({
            "source_id": source_id,
            "version": 1,
            "title": "T",
            "authors": ["A"],
            "published": "2024-01-01",
            "summary": "S",
        }))
        .unwrap();
        m.doi = doi.map(String::from);
        m
    }

    /// The merge endpoint's three faces: 404 for an unknown duplicate, 409 for
    /// a self-merge, and the happy path returning the receipt with the loser
    /// gone afterwards. The row-level matrix lives with the service/storage
    /// tests; this covers the HTTP mapping.
    #[tokio::test]
    async fn merge_maps_guards_to_404_409_and_returns_the_receipt() {
        let st = state();
        let winner = meta("arxiv:W", Some("10.1/x"));
        let loser = meta("local:L", Some("10.1/x"));
        st.with_conn(|conn| svc_paper::save_paper_metadata(conn, &winner, None))
            .unwrap();
        st.with_conn(|conn| svc_paper::save_paper_metadata(conn, &loser, None))
            .unwrap();
        let sfk_of = |sid: &str| -> i64 {
            st.with_conn(|conn| svc_paper::resolve_source_fk(conn, sid))
                .unwrap()
        };
        let (w, l) = (sfk_of("arxiv:W"), sfk_of("local:L"));

        let err = req(
            &st,
            "POST",
            &format!("/api/papers/sfk/{w}/merge"),
            Some(json!({ "loser_source_fk": 9999 })),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 404);

        let err = req(
            &st,
            "POST",
            &format!("/api/papers/sfk/{w}/merge"),
            Some(json!({ "loser_source_fk": w })),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 409, "self-merge must 409: {}", err.detail);

        let receipt = req(
            &st,
            "POST",
            &format!("/api/papers/sfk/{w}/merge"),
            Some(json!({ "loser_source_fk": l })),
        )
        .await
        .unwrap();
        assert_eq!(receipt["winner_source_id"], "arxiv:W");
        assert_eq!(receipt["merged_source_id"], "local:L");
        assert_eq!(receipt["versions_collapsed"], 1);

        // The duplicate root is gone from every read surface.
        let err = req(&st, "GET", &format!("/api/papers/sfk/{l}"), None)
            .await
            .unwrap_err();
        assert_eq!(err.status, 404);
        // And no DOI twin is suggested any more.
        let cands = req(
            &st,
            "GET",
            &format!("/api/papers/sfk/{w}/doi-candidates"),
            None,
        )
        .await
        .unwrap();
        assert_eq!(cands, json!({ "candidates": [] }));
    }

    /// The guards that run BEFORE any network call, so they are the testable part
    /// of the arm: unknown paper, and a paper with no arXiv source to fetch. The
    /// happy path needs a real arXiv fetch, and `arxiv_get`'s host allowlist
    /// rejects a loopback mock, so it isn't covered by an automated test here --
    /// same constraint `service::files`'s existing download tests document.
    #[tokio::test]
    async fn fetch_full_text_rejects_before_reaching_the_network() {
        let st = state();
        let err = req(&st, "POST", "/api/papers/arxiv:nope/full-text", None)
            .await
            .unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Paper arxiv:nope not found");

        // An arXiv paper saved without a `/pdf/` URL → refused as unfetchable.
        let mut no_url = meta("arxiv:2", None);
        no_url.source = Some("arxiv".into());
        st.with_conn(|conn| svc_paper::save_paper_metadata(conn, &no_url, None))
            .unwrap();
        let err = req(&st, "POST", "/api/papers/arxiv:2/full-text", None)
            .await
            .unwrap_err();
        assert_eq!(err.status, 400, "got {} / {}", err.status, err.detail);
        assert!(
            err.detail.contains("no arXiv PDF URL"),
            "unexpected detail: {}",
            err.detail
        );

        // A non-arXiv paper is refused on its source_id namespace, and the
        // message names the paper rather than leaving a blank. The PROVIDER
        // column is deliberately not consulted — it arrives blank on some
        // import paths and defaults to 'arxiv' for pre-migration rows.
        let mut crossref = meta("doi:10.1/z", None);
        crossref.source = Some("crossref".into());
        st.with_conn(|conn| svc_paper::save_paper_metadata(conn, &crossref, None))
            .unwrap();
        let err = req(&st, "POST", "/api/papers/doi:10.1%2Fz/full-text", None)
            .await
            .unwrap_err();
        assert_eq!(err.status, 400);
        assert!(
            err.detail.contains("doi:10.1/z is not an arXiv paper"),
            "unexpected detail: {}",
            err.detail
        );
    }

    /// An already-indexed paper short-circuits instead of re-fetching; `force`
    /// is what gets past it.
    #[tokio::test]
    async fn fetch_full_text_skips_an_already_indexed_paper() {
        let st = state();
        st.with_conn(|conn| svc_paper::save_paper_metadata(conn, &meta("arxiv:3", None), None))
            .unwrap();
        st.with_conn(|conn| svc_paper::set_full_text(conn, "arxiv:3", 1, "already here"))
            .unwrap();
        let out = req(&st, "POST", "/api/papers/arxiv:3/full-text", None)
            .await
            .unwrap();
        assert_eq!(out["indexed"], json!(false));

        // With force=true the skip no longer applies, so it falls through to the
        // unfetchable-URL refusal rather than returning `indexed: false`.
        let err = req(
            &st,
            "POST",
            "/api/papers/arxiv:3/full-text?force=true",
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 400);
    }

    /// `full_text` is the FTS payload, not a display field — once ingestion has
    /// run it is megabytes of TeX per paper.
    #[tokio::test]
    async fn paper_responses_omit_the_indexed_full_text() {
        let st = state();
        st.with_conn(|conn| svc_paper::save_paper_metadata(conn, &meta("arxiv:4", None), None))
            .unwrap();
        st.with_conn(|conn| svc_paper::set_full_text(conn, "arxiv:4", 1, "the whole tex body"))
            .unwrap();

        let one = req(&st, "GET", "/api/papers/arxiv:4", None).await.unwrap();
        assert!(one.get("full_text").is_none(), "leaked full_text: {one}");
        assert_eq!(one["downloaded_source"], json!(true));

        let listed = req(&st, "GET", "/api/papers", None).await.unwrap();
        assert!(
            listed["papers"][0].get("full_text").is_none(),
            "leaked full_text: {listed}"
        );

        let searched = req(&st, "GET", "/api/papers/search?q=whole%20tex%20body", None)
            .await
            .unwrap();
        assert!(
            searched["papers"][0].get("full_text").is_none(),
            "leaked full_text: {searched}"
        );
        assert_eq!(searched["papers"][0]["downloaded_source"], json!(true));
    }

    #[tokio::test]
    async fn doi_candidates_missing_paper_is_404() {
        let err = req(&state(), "GET", "/api/papers/sfk/999/doi-candidates", None)
            .await
            .unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Paper 999 not found");
    }

    #[tokio::test]
    async fn doi_candidates_matches_same_doi_across_sources() {
        let st = state();
        let (arxiv_sid, _) = st
            .with_conn(|conn| {
                svc_paper::save_paper_metadata(conn, &meta("arxiv:1", Some("10.1/x")), None)
            })
            .unwrap();
        let (openalex_sid, _) = st
            .with_conn(|conn| {
                svc_paper::save_paper_metadata(conn, &meta("openalex:W1", Some("10.1/x")), None)
            })
            .unwrap();
        let arxiv_fk = st
            .with_conn(|conn| svc_paper::ensure_paper_root(conn, &arxiv_sid))
            .unwrap();
        st.with_conn(|conn| svc_paper::ensure_paper_root(conn, &openalex_sid))
            .unwrap();

        let body = req(
            &st,
            "GET",
            &format!("/api/papers/sfk/{arxiv_fk}/doi-candidates"),
            None,
        )
        .await
        .unwrap();
        let candidates = body["candidates"].as_array().unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0]["source_id"], "openalex:W1");
    }

    #[tokio::test]
    async fn versions_missing_is_404() {
        let err = req(&state(), "GET", "/api/papers/sfk/999/versions", None)
            .await
            .unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Paper 999 not found");
    }

    #[tokio::test]
    async fn by_sfk_missing_is_404_both_branches() {
        let st = state();
        assert_eq!(
            req(&st, "GET", "/api/papers/sfk/999", None)
                .await
                .unwrap_err()
                .detail,
            "Paper 999 not found"
        );
        // version branch: unknown sfk is the typed miss (source_id resolves to None).
        assert_eq!(
            req(&st, "GET", "/api/papers/sfk/999?version=2", None)
                .await
                .unwrap_err()
                .detail,
            "Paper 999 not found"
        );
    }

    #[tokio::test]
    async fn non_integer_sfk_is_422() {
        let err = req(&state(), "GET", "/api/papers/sfk/abc/versions", None)
            .await
            .unwrap_err();
        assert_eq!(err.status, 422);
    }

    #[tokio::test]
    async fn search_short_query_is_422() {
        let err = req(&state(), "GET", "/api/papers/search?q=ab", None)
            .await
            .unwrap_err();
        assert_eq!(err.status, 422);
        assert_eq!(
            err.detail,
            "Query must contain at least 3 non-whitespace characters"
        );
    }

    #[tokio::test]
    async fn search_whitespace_only_query_is_422() {
        // q is trimmed before the length check (matches app.py `q.strip()`).
        let err = req(&state(), "GET", "/api/papers/search?q=%20%20a%20%20", None)
            .await
            .unwrap_err();
        assert_eq!(err.status, 422);
    }

    /// FTS5 reads `-` as column-filter syntax, so a hyphenated query raised and
    /// surfaced as an empty result set. Pinned at the HTTP boundary too, since
    /// this is the path the GUI search box takes.
    #[tokio::test]
    async fn search_matches_a_hyphenated_query() {
        let st = state();
        st.with_conn(|conn| svc_paper::save_paper_metadata(conn, &meta("arxiv:5", None), None))
            .unwrap();
        st.with_conn(|conn| {
            svc_paper::set_full_text(conn, "arxiv:5", 1, "an encoder-decoder, state-of-the-art")
        })
        .unwrap();

        for q in ["encoder-decoder", "state-of-the-art", "encoder decoder"] {
            let out = req(&st, "GET", &format!("/api/papers/search?q={q}"), None)
                .await
                .unwrap();
            assert_eq!(
                out["papers"][0]["source_id"],
                json!("arxiv:5"),
                "query {q:?}"
            );
        }
    }

    /// Adversarial FTS5 syntax: unterminated quote, dangling operators.
    #[tokio::test]
    async fn search_adversarial_queries_do_not_error() {
        let st = state();
        st.with_conn(|conn| svc_paper::save_paper_metadata(conn, &meta("arxiv:6", None), None))
            .unwrap();
        st.with_conn(|conn| svc_paper::set_full_text(conn, "arxiv:6", 1, "an abc term"))
            .unwrap();

        // %22abc decodes to `"abc` (unterminated quote): still finds the seeded paper.
        let out = req(&st, "GET", "/api/papers/search?q=%22abc", None)
            .await
            .unwrap();
        assert_eq!(out["papers"][0]["source_id"], json!("arxiv:6"));

        // AND%20OR decodes to `AND OR` (dangling operators, nothing searchable):
        // succeeds with an empty result rather than erroring.
        let out = req(&st, "GET", "/api/papers/search?q=AND%20OR", None)
            .await
            .unwrap();
        assert_eq!(out["papers"], json!([]));
    }

    #[tokio::test]
    async fn search_empty_db_wraps_empty_array() {
        assert_eq!(
            req(&state(), "GET", "/api/papers/search?q=manifold", None)
                .await
                .unwrap(),
            json!({ "papers": [] })
        );
    }

    #[tokio::test]
    async fn repair_missing_paper_is_404() {
        let body = json!({"title":"T","authors":["A"],"published":"2024-01-01","summary":"s"});
        let err = req(&state(), "PUT", "/api/papers/sfk/999", Some(body))
            .await
            .unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Paper 999 not found");
    }

    #[tokio::test]
    async fn repair_bad_date_is_422() {
        let st = state();
        // The date is parsed after the existence check, so the paper must exist
        // to reach it — an absent one answers 404 first (repair_missing_paper_is_404).
        st.with_conn(|conn| svc_paper::save_paper_metadata(conn, &meta("arxiv:8", None), None))
            .unwrap();
        let fk = st
            .with_conn(|conn| svc_paper::ensure_paper_root(conn, "arxiv:8"))
            .unwrap();

        let body = json!({"title":"T","authors":["A"],"published":"not-a-date","summary":"s"});
        let err = req(&st, "PUT", &format!("/api/papers/sfk/{fk}"), Some(body))
            .await
            .unwrap_err();
        assert_eq!(err.status, 422);
    }

    #[tokio::test]
    async fn repair_validators_reject_blank_title_empty_authors_and_empty_doi() {
        let st = state();
        // svc_paper::repair_paper validates, so the paper must exist to reach it.
        st.with_conn(|conn| svc_paper::save_paper_metadata(conn, &meta("arxiv:7", None), None))
            .unwrap();
        let fk = st
            .with_conn(|conn| svc_paper::ensure_paper_root(conn, "arxiv:7"))
            .unwrap();
        let path = format!("/api/papers/sfk/{fk}");

        let blank_title = json!({"title":"   ","authors":["A"],"published":"2024-01-01"});
        assert_eq!(
            req(&st, "PUT", &path, Some(blank_title))
                .await
                .unwrap_err()
                .status,
            422
        );
        let no_authors = json!({"title":"T","authors":["  ",""],"published":"2024-01-01"});
        assert_eq!(
            req(&st, "PUT", &path, Some(no_authors))
                .await
                .unwrap_err()
                .status,
            422
        );
        let empty_doi = json!({"title":"T","authors":["A"],"published":"2024-01-01","doi":""});
        assert_eq!(
            req(&st, "PUT", &path, Some(empty_doi))
                .await
                .unwrap_err()
                .status,
            422
        );
    }

    #[tokio::test]
    async fn by_sfk_invalid_version_is_422() {
        let st = state();
        assert_eq!(
            req(&st, "GET", "/api/papers/sfk/1?version=abc", None)
                .await
                .unwrap_err()
                .status,
            422
        );
        assert_eq!(
            req(&st, "GET", "/api/papers/sfk/1?version=0", None)
                .await
                .unwrap_err()
                .status,
            422
        );
    }

    #[tokio::test]
    async fn remove_from_projects_empty_is_ok() {
        assert_eq!(
            req(&state(), "DELETE", "/api/papers/sfk/999/projects", None)
                .await
                .unwrap(),
            json!({ "ok": true, "removed_from_projects": [] })
        );
    }
}
