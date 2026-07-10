//! `/api/share` routes — Phase-0 quarantined CRDT "shared projects". A second
//! front door beside `api`: `share_api` resolves `ShareState` alongside the
//! canonical `AppState`. Publishing only READS `papers.db` (through linxiv-share's
//! read-only `publish`); the CRDT docs live under the injected share directory.
//!
//! `route()` and its callers (the dev_server bin, the linxiv:// protocol handler)
//! are unchanged — this dispatcher is invoked only via the `share_api` command.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::Mutex;

/// Cap on a single network op (mint ticket / fetch); past it the request returns
/// 504 and releases the node lock so shutdown is not blocked.
const SHARE_NET_TIMEOUT: Duration = Duration::from_secs(30);

use linxiv_share::{build_shared_project, save, ShareError, ShareNode, ShareStore, ShareTicket};

use crate::route::{parse_query, path_i64, split_segments, ApiError, ApiRequest, ReqCtx};
use crate::state::AppState;

/// Managed beside `AppState` (never a field of it). Owns the injected
/// `ShareStore` over the share directory (production `config::data_dir()/share`,
/// a tempdir in tests) and, in the packaged app, the iroh `ShareNode` that serves
/// and fetches over the network. `node` is `None` in the store-only Phase-0 unit
/// tests (no socket); the network arms then return 503.
pub struct ShareState {
    store: ShareStore,
    // `Option` so store-only tests skip the async bind; `Mutex<Arc>` so a network
    // arm clones the `Arc` and drops the guard before its `.await`, and `shutdown`
    // can take the node out of a shared (`tauri::State`) value.
    node: Mutex<Option<Arc<ShareNode>>>,
}

impl ShareState {
    /// Store-only state (no network node). Used by the Phase-0 sync unit tests.
    pub fn new(share_dir: impl Into<PathBuf>) -> Self {
        Self {
            store: ShareStore::new(share_dir),
            node: Mutex::new(None),
        }
    }

    /// Full state with a live iroh node serving the same share directory.
    pub fn with_node(share_dir: impl Into<PathBuf>, node: ShareNode) -> Self {
        Self {
            store: ShareStore::new(share_dir),
            node: Mutex::new(Some(Arc::new(node))),
        }
    }

    /// Tear the iroh endpoint + router down explicitly (Drop is not enough — the
    /// async close must run). Idempotent: a second call finds `None` and no-ops.
    pub async fn shutdown(&self) -> Result<(), ShareError> {
        if let Some(node) = self.node.lock().await.take() {
            node.shutdown().await?;
        }
        Ok(())
    }
}

impl From<ShareError> for ApiError {
    fn from(e: ShareError) -> Self {
        let status = match &e {
            ShareError::NotFound(_) => 404,
            ShareError::Core(c) => c.http_status(),
            ShareError::Transport(_) => 502,
            ShareError::Io(_) | ShareError::Crdt(_) => 500,
        };
        ApiError::new(status, e.to_string())
    }
}

/// The Tauri command the webview invokes for `/api/share/*`. Mirrors `api`'s
/// `{method, path, body}` shape but resolves `ShareState` alongside `AppState`.
#[tauri::command]
pub async fn share_api(
    state: tauri::State<'_, AppState>,
    share: tauri::State<'_, ShareState>,
    req: ApiRequest,
) -> Result<Value, ApiError> {
    let (raw_path, raw_query) = req.path.split_once('?').unwrap_or((req.path.as_str(), ""));
    let segs = split_segments(raw_path);
    let query = parse_query(raw_query);
    let s: Vec<&str> = segs.iter().map(String::as_str).collect();
    let ctx = ReqCtx {
        method: req.method.as_str(),
        segs: &s,
        query: &query,
        body: req.body.as_ref(),
    };

    // Network arms own iroh `.await`s, so they live in the async command rather
    // than the sync `handle` dispatcher the Phase-0 store arms share.
    match (ctx.method, ctx.segs) {
        ("POST", ["api", "share", "project", id, "ticket"]) => {
            return ticket(state.inner(), share.inner(), id).await
        }
        ("POST", ["api", "share", "join"]) => return join(share.inner(), ctx.body).await,
        _ => {}
    }
    handle(state.inner(), share.inner(), &ctx).unwrap_or_else(|| Err(ApiError::not_routed()))
}

/// Match a synchronous (store-only) `/api/share/*` request. Returns `None` (no
/// arm) so the command can surface the same not-routed sentinel `route()` uses.
/// The async network arms (ticket/join) are matched in `share_api` instead.
pub(crate) fn handle(
    state: &AppState,
    share: &ShareState,
    ctx: &ReqCtx<'_>,
) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("GET", ["api", "share", "projects"]) => Some(list_shared(share)),
        ("GET", ["api", "share", "received"]) => Some(list_received(share)),
        ("GET", ["api", "share", "received", id]) => Some(get_received(share, id)),
        ("POST", ["api", "share", "project", id, "publish"]) => Some(publish(state, share, id)),
        _ => None,
    }
}

/// `GET /api/share/projects` — summaries of every published shared project.
fn list_shared(share: &ShareState) -> Result<Value, ApiError> {
    let out: Vec<Value> = share
        .store
        .list_shared()?
        .into_iter()
        .map(|s| {
            json!({
                "share_id": s.share_id,
                "name": s.name,
                "paper_count": s.paper_count,
                "note_count": s.note_count,
                "tag_count": s.tag_count,
            })
        })
        .collect();
    Ok(json!({ "shared_projects": out }))
}

/// `GET /api/share/received` — summaries of every mirror materialized by `join`.
fn list_received(share: &ShareState) -> Result<Value, ApiError> {
    let out: Vec<Value> = linxiv_share::ShareNode::list_received(share.store.share_dir())?
        .into_iter()
        .map(|s| {
            json!({
                "share_id": s.share_id,
                "name": s.name,
                "paper_count": s.paper_count,
                "note_count": s.note_count,
                "tag_count": s.tag_count,
            })
        })
        .collect();
    Ok(json!({ "received": out }))
}

/// `GET /api/share/received/{id}` — the full subgraph of one received mirror.
fn get_received(share: &ShareState, id: &str) -> Result<Value, ApiError> {
    let sp = linxiv_share::ShareNode::received(share.store.share_dir(), id)?;
    Ok(json!({
        "share_id": sp.share_id,
        "name": sp.name,
        "description": sp.description,
        "color": sp.color,
        "tags": sp.tags,
        "papers": sp.papers.iter().map(|p| json!({
            "source_id": p.source_id,
            "version": p.version,
            "title": p.title,
            "summary": p.summary,
            "authors": p.authors,
            "tags": p.tags,
        })).collect::<Vec<_>>(),
        "notes": sp.notes.iter().map(|n| json!({
            "id": n.id,
            "title": n.title,
            "body": n.body,
            "created_at": n.created_at,
            "updated_at": n.updated_at,
        })).collect::<Vec<_>>(),
    }))
}

/// `POST /api/share/project/{id}/publish` — snapshot a canonical project into the
/// CRDT store (read-only over the canonical connection) and return its share_id.
fn publish(state: &AppState, share: &ShareState, id: &str) -> Result<Value, ApiError> {
    let project_id = path_i64(id)?;
    // Build the snapshot under the DB lock; write the CRDT doc to disk outside it.
    let sp = state.with_conn(|conn| build_shared_project(conn, project_id))?;
    save(share.store.share_dir(), &sp)?;
    Ok(json!({ "share_id": sp.share_id }))
}

/// `POST /api/share/project/{id}/ticket` — ensure the project is published
/// (Phase-0 publish, read-only over the canonical connection), then mint a
/// pasteable ticket carrying the sender's address + an unguessable capability.
async fn ticket(state: &AppState, share: &ShareState, id: &str) -> Result<Value, ApiError> {
    let project_id = path_i64(id)?;
    let sp = state.with_conn(|conn| build_shared_project(conn, project_id))?;
    save(share.store.share_dir(), &sp)?;

    // Clone the node Arc out from under the lock, then release it: the 30s network
    // op must not hold the guard `shutdown()` also needs.
    let node = share
        .node
        .lock()
        .await
        .clone()
        .ok_or_else(node_unavailable)?;
    let ticket = tokio::time::timeout(SHARE_NET_TIMEOUT, node.ticket(&sp.share_id))
        .await
        .map_err(|_| ApiError::new(504, "share ticket timed out"))??;
    Ok(json!({ "ticket": ticket.to_string() }))
}

/// `POST /api/share/join` — dial the ticket's sender, fetch the CRDT doc, and
/// materialize it as a read-only mirror under the receiver's share dir. Returns
/// the resulting shared-project summary.
async fn join(share: &ShareState, body: Option<&Value>) -> Result<Value, ApiError> {
    let raw = body
        .and_then(|b| b.get("ticket"))
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::new(422, "missing `ticket` in body"))?;
    // A malformed/unparseable ticket is the caller's bad input, not a server fault.
    let ticket: ShareTicket = raw
        .parse()
        .map_err(|e: ShareError| ApiError::new(400, e.to_string()))?;

    // Release the node lock before the 30s fetch so `shutdown()` can take it.
    let node = share
        .node
        .lock()
        .await
        .clone()
        .ok_or_else(node_unavailable)?;
    let sp = tokio::time::timeout(
        SHARE_NET_TIMEOUT,
        node.fetch(&ticket, share.store.share_dir()),
    )
    .await
    .map_err(|_| ApiError::new(504, "share join timed out"))?
    .map_err(fetch_error)?;
    Ok(json!({
        "share_id": sp.share_id,
        "name": sp.name,
        "paper_count": sp.papers.len(),
        "note_count": sp.notes.len(),
        "tag_count": sp.tags.len(),
    }))
}

fn node_unavailable() -> ApiError {
    ApiError::new(503, "share transport not initialized")
}

/// Map a `fetch` failure to a status: a refused/unknown capability is a 404 (the
/// peer answered, the doc just isn't served to us); any other failure during the
/// live dial is an upstream/transport fault, surfaced as 502 — never a blanket 500.
fn fetch_error(e: ShareError) -> ApiError {
    match e {
        ShareError::NotFound(_) => ApiError::new(404, e.to_string()),
        _ => ApiError::new(502, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use chrono::NaiveDate;
    use linxiv_core::models::{PaperIn, ProjectIn};
    use linxiv_core::service::{paper as paper_svc, project as project_svc};
    use linxiv_core::storage;

    // Seed a canonical in-memory DB via the real service WRITE APIs, then hand the
    // connection to AppState. Returns (AppState, project_id).
    fn seeded_state() -> (AppState, i64) {
        let mut conn = storage::open_in_memory().unwrap();
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
            &pin("arxiv:1", "First", &["Alice"], &["ml"]),
            None,
        )
        .unwrap();
        paper_svc::upsert(
            &mut conn,
            &pin("arxiv:2", "Second", &["Bob"], &["cv"]),
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

        let state = AppState::from_parts(conn, std::env::temp_dir(), std::env::temp_dir());
        (state, project_id)
    }

    fn dispatch(
        state: &AppState,
        share: &ShareState,
        method: &str,
        path: &str,
    ) -> Result<Value, ApiError> {
        let segs = split_segments(path);
        let query: HashMap<String, String> = HashMap::new();
        let s: Vec<&str> = segs.iter().map(String::as_str).collect();
        let ctx = ReqCtx {
            method,
            segs: &s,
            query: &query,
            body: None,
        };
        handle(state, share, &ctx).expect("share arm matched")
    }

    #[test]
    fn list_publish_list_envelopes() {
        let (state, pid) = seeded_state();
        let dir = tempfile::tempdir().unwrap();
        let share = ShareState::new(dir.path());

        // Empty before any publish.
        assert_eq!(
            dispatch(&state, &share, "GET", "/api/share/projects").unwrap(),
            json!({ "shared_projects": [] })
        );

        // Publish returns the project id as the share_id.
        assert_eq!(
            dispatch(
                &state,
                &share,
                "POST",
                &format!("/api/share/project/{pid}/publish")
            )
            .unwrap(),
            json!({ "share_id": pid.to_string() })
        );

        // The summary now lists the published project.
        let listed = dispatch(&state, &share, "GET", "/api/share/projects").unwrap();
        assert_eq!(
            listed,
            json!({
                "shared_projects": [{
                    "share_id": pid.to_string(),
                    "name": "My Project",
                    "paper_count": 2,
                    "note_count": 0,
                    "tag_count": 2,
                }]
            })
        );
    }

    #[test]
    fn publish_missing_project_is_404() {
        let (state, _pid) = seeded_state();
        let dir = tempfile::tempdir().unwrap();
        let share = ShareState::new(dir.path());

        let err = dispatch(&state, &share, "POST", "/api/share/project/9999/publish").unwrap_err();
        assert_eq!(err.status, 404);
    }

    // Needs one bound endpoint to resolve its own loopback addr; relays/discovery
    // are off (bind_offline), so it never contacts an external host — gated like
    // the crate's network tests (multi-thread runtime, no n0 relay).
    #[tokio::test(flavor = "multi_thread")]
    async fn ticket_route_mints_parseable_ticket() {
        let (state, pid) = seeded_state();
        let dir = tempfile::tempdir().unwrap();
        let node = ShareNode::bind_offline(dir.path()).await.unwrap();
        let share = ShareState::with_node(dir.path(), node);

        let resp = ticket(&state, &share, &pid.to_string()).await.unwrap();
        let encoded = resp.get("ticket").and_then(Value::as_str).unwrap();
        // The minted ticket round-trips through the pasteable encoding.
        let parsed: ShareTicket = encoded.parse().unwrap();
        assert_eq!(parsed.to_string(), encoded);

        share.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn join_rejects_bad_ticket_with_400() {
        let dir = tempfile::tempdir().unwrap();
        let share = ShareState::new(dir.path());

        let body = json!({ "ticket": "not-a-valid-ticket" });
        let err = join(&share, Some(&body)).await.unwrap_err();
        assert_eq!(err.status, 400);
    }
}
