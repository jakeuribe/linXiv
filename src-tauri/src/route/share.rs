//! `/api/share` routes — Phase-0 quarantined CRDT "shared projects". A second
//! front door beside `api`: `share_api` resolves `ShareState` alongside the
//! canonical `AppState`. Publishing only READS `papers.db` (through linxiv-share's
//! read-only `publish`); the CRDT docs live under the injected share directory.
//!
//! `route()` and its callers (the dev_server bin, the linxiv:// protocol handler)
//! are unchanged — this dispatcher is invoked only via the `share_api` command.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::Mutex;

/// Cap on a single network op (mint ticket / fetch); past it the request returns
/// 504 and releases the node lock so shutdown is not blocked.
pub(crate) const SHARE_NET_TIMEOUT: Duration = Duration::from_secs(30);

use linxiv_core::service::project as project_svc;
use linxiv_share::{
    build_shared_project, doc_path, received_dir, save, valid_share_id, ShareError, ShareNode,
    ShareStore, ShareTicket,
};

use crate::route::{parse_query, path_i64, split_segments, ApiError, ApiRequest, ReqCtx};
use crate::share_sync;
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
    // Entries persist for the process lifetime.
    write_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl ShareState {
    /// Store-only state (no network node). Used by the Phase-0 sync unit tests.
    pub fn new(share_dir: impl Into<PathBuf>) -> Self {
        Self {
            store: ShareStore::new(share_dir),
            node: Mutex::new(None),
            write_locks: Mutex::new(HashMap::new()),
        }
    }

    /// Full state with a live iroh node serving the same share directory.
    pub fn with_node(share_dir: impl Into<PathBuf>, node: ShareNode) -> Self {
        Self {
            store: ShareStore::new(share_dir),
            node: Mutex::new(Some(Arc::new(node))),
            write_locks: Mutex::new(HashMap::new()),
        }
    }

    /// Share directory backing this state (docs + settings/ticket sidecars).
    pub fn share_dir(&self) -> &Path {
        self.store.share_dir()
    }

    /// Clone the live node out from under the lock (`None` while store-only).
    pub(crate) async fn node(&self) -> Option<Arc<ShareNode>> {
        self.node.lock().await.clone()
    }

    /// Acquire the write lock for a specific share. Returns a guard on the per-share-id lock.
    pub(crate) async fn lock_writes(&self, share_id: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let mut locks = self.write_locks.lock().await;
        let arc = locks
            .entry(share_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        drop(locks);
        arc.lock_owned().await
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
        ("POST", ["api", "share", "project", id, "publish"]) => {
            return publish(state.inner(), share.inner(), id).await
        }
        ("POST", ["api", "share", id, "unpublish"]) => return unpublish(share.inner(), id).await,
        ("POST", ["api", "share", "received", id, "import"]) => {
            if !valid_share_id(id) {
                return Err(ApiError::new(404, format!("share {id:?} not found")));
            }
            let _lock = share.lock_writes(id).await;
            return share_sync::import_received(state.inner(), share.inner().share_dir(), id)
                .map(|fk| json!({ "project_fk": fk }));
        }
        ("POST", ["api", "share", "received", id, "leave"]) => {
            return leave(share.inner(), id).await
        }
        ("POST", ["api", "share", id, "sync"]) => {
            return share_sync::sync_share(state.inner(), share.inner(), id).await
        }
        ("PUT" | "POST", ["api", "share", id, "settings"]) => {
            return put_settings(share.inner(), id, ctx.body).await
        }
        _ => {}
    }
    handle(state.inner(), share.inner(), &ctx).unwrap_or_else(|| Err(ApiError::not_routed()))
}

/// Match a synchronous (no-await) `/api/share/*` request. Returns `None` (no
/// arm) so the command can surface the same not-routed sentinel `route()` uses.
/// The async network arms (ticket/join/sync) are matched in `share_api` instead.
pub(crate) fn handle(
    state: &AppState,
    share: &ShareState,
    ctx: &ReqCtx<'_>,
) -> Option<Result<Value, ApiError>> {
    // Static segments "projects" and "received" shadow a share literally named as such.
    match (ctx.method, ctx.segs) {
        ("GET", ["api", "share", "projects"]) => Some(list_shared(state, share)),
        ("GET", ["api", "share", "received"]) => Some(list_received(state, share)),
        ("GET", ["api", "share", "received", id]) => Some(get_received(share, id)),
        ("GET", ["api", "share", id, "settings"]) => Some(get_settings(share, id)),
        _ => None,
    }
}

/// Doc-file mtime = the last local save/fetch of the CRDT doc, as ISO 8601.
fn synced_at(doc: &Path) -> Value {
    match std::fs::metadata(doc).and_then(|m| m.modified()) {
        Ok(t) => json!(chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339()),
        Err(_) => Value::Null,
    }
}

fn summary_json(s: &linxiv_share::SharedSummary, doc: &Path, share_dir: &Path) -> Value {
    json!({
        "share_id": s.share_id,
        "name": s.name,
        "paper_count": s.paper_count,
        "note_count": s.note_count,
        "tag_count": s.tag_count,
        "synced_at": synced_at(doc),
        "paused": share_sync::load_settings(share_dir, &s.share_id).paused,
    })
}

/// `GET /api/share/projects` — summaries of every published shared project.
fn list_shared(state: &AppState, share: &ShareState) -> Result<Value, ApiError> {
    let dir = share.store.share_dir();
    let mut out = Vec::new();
    for s in share.store.list_shared()? {
        let mut v = summary_json(&s, &doc_path(dir, &s.share_id), dir);
        let fk = state.with_conn(|c| project_svc::find_by_share_id(c, &s.share_id))?;
        v["project_fk"] = json!(fk);
        out.push(v);
    }
    Ok(json!({ "shared_projects": out }))
}

/// `GET /api/share/received` — summaries of every mirror materialized by `join`,
/// each carrying the `project_fk` of the linked local project (null pre-import).
fn list_received(state: &AppState, share: &ShareState) -> Result<Value, ApiError> {
    let dir = share.store.share_dir();
    let rec = received_dir(dir);
    let mut out = Vec::new();
    for s in linxiv_share::ShareNode::list_received(dir)? {
        let mut v = summary_json(&s, &doc_path(&rec, &s.share_id), dir);
        let fk = state.with_conn(|c| project_svc::find_by_share_id(c, &s.share_id))?;
        v["project_fk"] = json!(fk);
        out.push(v);
    }
    Ok(json!({ "received": out }))
}

/// `GET /api/share/{id}/settings` — the per-share sidecar (defaults if unset).
fn get_settings(share: &ShareState, id: &str) -> Result<Value, ApiError> {
    if !valid_share_id(id) {
        return Err(ApiError::new(404, format!("share {id:?} not found")));
    }
    let dir = share.share_dir();
    if !doc_path(dir, id).is_file() && !doc_path(&received_dir(dir), id).is_file() {
        return Err(ApiError::new(404, format!("share {id:?} not found")));
    }
    Ok(serde_json::to_value(share_sync::load_settings(dir, id)).unwrap())
}

/// `PUT /api/share/{id}/settings` — partial update over the current sidecar.
async fn put_settings(
    share: &ShareState,
    id: &str,
    body: Option<&Value>,
) -> Result<Value, ApiError> {
    if !valid_share_id(id) {
        return Err(ApiError::new(404, format!("share {id:?} not found")));
    }
    let dir = share.share_dir();
    if !doc_path(dir, id).is_file() && !doc_path(&received_dir(dir), id).is_file() {
        return Err(ApiError::new(404, format!("share {id:?} not found")));
    }
    let _lock = share.lock_writes(id).await;
    let mut s = share_sync::load_settings(dir, id);
    if let Some(p) = body.and_then(|b| b.get("paused")) {
        s.paused = p
            .as_bool()
            .ok_or_else(|| ApiError::new(422, "`paused` must be a boolean"))?;
    }
    if let Some(d) = body.and_then(|b| b.get("direction")) {
        s.direction =
            serde_json::from_value::<share_sync::SyncDirection>(d.clone()).map_err(|_| {
                ApiError::new(
                    422,
                    "direction must be one of two_way, shared_to_local, local_to_shared",
                )
            })?;
    }
    share_sync::save_settings(dir, id, &s)
        .map_err(|e| ApiError::new(500, format!("could not persist share settings: {e}")))?;
    Ok(serde_json::to_value(s).unwrap())
}

/// `<doc>.unpublished` — where `unpublish` parks a doc's CRDT history.
fn unpublished_path(doc: &Path) -> PathBuf {
    let mut p = doc.as_os_str().to_owned();
    p.push(".unpublished");
    PathBuf::from(p)
}

/// Move a parked doc back to the live name when no live doc exists.
fn restore_unpublished(dir: &Path, share_id: &str) {
    let doc = doc_path(dir, share_id);
    if !doc.is_file() {
        let parked = unpublished_path(&doc);
        if parked.is_file() {
            let _ = std::fs::rename(&parked, &doc);
        }
    }
}

/// `POST /api/share/{id}/unpublish` — park the published doc as
/// `<id>.automerge.unpublished` and delete its settings sidecar.
async fn unpublish(share: &ShareState, id: &str) -> Result<Value, ApiError> {
    if !valid_share_id(id) {
        return Err(ApiError::new(404, format!("share {id:?} not found")));
    }
    let dir = share.share_dir();
    let doc = doc_path(dir, id);
    let _lock = share.lock_writes(id).await;
    if !doc.is_file() {
        return Err(ApiError::new(404, format!("share {id:?} not found")));
    }
    std::fs::rename(&doc, unpublished_path(&doc))
        .map_err(|e| ApiError::new(500, format!("could not unpublish: {e}")))?;
    let _ = std::fs::remove_file(share_sync::settings_path(dir, id));
    Ok(json!({ "unpublished": true, "share_id": id }))
}

/// `POST /api/share/received/{id}/leave` — delete the mirror + ticket + settings.
async fn leave(share: &ShareState, id: &str) -> Result<Value, ApiError> {
    if !valid_share_id(id) {
        return Err(ApiError::new(404, format!("share {id:?} not found")));
    }
    let dir = share.share_dir();
    let mirror = doc_path(&received_dir(dir), id);
    let _lock = share.lock_writes(id).await;
    if !mirror.is_file() {
        return Err(ApiError::new(
            404,
            format!("received share {id:?} not found"),
        ));
    }
    std::fs::remove_file(&mirror)
        .map_err(|e| ApiError::new(500, format!("could not leave share: {e}")))?;
    let _ = std::fs::remove_file(share_sync::ticket_path(dir, id));
    let _ = std::fs::remove_file(share_sync::settings_path(dir, id));
    Ok(json!({ "left": true }))
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
            "id": n.uuid,
            "title": n.title,
            "body": n.body,
            "created_at": n.created_at,
            "updated_at": n.updated_at,
        })).collect::<Vec<_>>(),
    }))
}

/// `POST /api/share/project/{id}/publish` — snapshot a canonical project into the
/// CRDT store (read-only over the canonical connection) and return its share_id.
async fn publish(state: &AppState, share: &ShareState, id: &str) -> Result<Value, ApiError> {
    let project_id = path_i64(id)?;
    let sp = state.with_conn(|conn| build_shared_project(conn, project_id))?;
    let dir = share.store.share_dir();
    if doc_path(&received_dir(dir), &sp.share_id).is_file() {
        return Err(ApiError::new(
            409,
            "project is linked to a received share; leave the share before publishing",
        ));
    }
    let _lock = share.lock_writes(&sp.share_id).await;
    restore_unpublished(dir, &sp.share_id);
    save(dir, &sp)?;
    if let Some(node) = share.node().await {
        node.refresh(&sp.share_id).await?;
    }
    Ok(json!({ "share_id": sp.share_id }))
}

// Clone the node Arc out from under the lock, then release it: the 30s network
// op must not hold the guard `shutdown()` also needs.
async fn live_node(share: &ShareState) -> Result<Arc<ShareNode>, ApiError> {
    share
        .node
        .lock()
        .await
        .clone()
        .ok_or_else(|| ApiError::new(503, "share transport not initialized"))
}

/// `POST /api/share/project/{id}/ticket` — ensure the project is published
/// (Phase-0 publish, read-only over the canonical connection), then mint a
/// pasteable ticket carrying the sender's address + share id; access is gated
/// by whether that id is currently published, not a per-recipient secret.
async fn ticket(state: &AppState, share: &ShareState, id: &str) -> Result<Value, ApiError> {
    let project_id = path_i64(id)?;
    let sp = state.with_conn(|conn| build_shared_project(conn, project_id))?;
    let dir = share.store.share_dir();
    if doc_path(&received_dir(dir), &sp.share_id).is_file() {
        return Err(ApiError::new(
            409,
            "project is linked to a received share; leave the share before publishing",
        ));
    }
    let _lock = share.lock_writes(&sp.share_id).await;
    restore_unpublished(dir, &sp.share_id);
    save(dir, &sp)?;

    let node = live_node(share).await?;
    let ticket = tokio::time::timeout(SHARE_NET_TIMEOUT, node.ticket(&sp.share_id))
        .await
        .map_err(|_| ApiError::new(504, "share ticket timed out"))??;
    Ok(json!({ "ticket": ticket.to_string(), "share_id": sp.share_id }))
}

/// `POST /api/share/join` — dial the ticket's sender, fetch the CRDT doc, and
/// materialize it as a read-only mirror under the receiver's share dir. Returns
/// the resulting shared-project summary.
async fn join(share: &ShareState, body: Option<&Value>) -> Result<Value, ApiError> {
    let raw = body
        .and_then(|b| b.get("ticket"))
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::new(422, "missing `ticket` in body"))?;
    let ticket: ShareTicket = raw.parse().map_err(|e| {
        ApiError::new(
            400,
            format!("invalid ticket (tickets from older linXiv versions are no longer valid): {e}"),
        )
    })?;

    let node = live_node(share).await?;
    // Held across the fetch, covering the mirror write for this share id.
    let _lock = share.lock_writes(ticket.project_id()).await;
    let sp = tokio::time::timeout(
        SHARE_NET_TIMEOUT,
        node.fetch(&ticket, share.store.share_dir()),
    )
    .await
    .map_err(|_| ApiError::new(504, "share join timed out"))?
    .map_err(fetch_error)?;
    // Ticket sidecar: re-sync needs the origin address. Failed write is logged.
    let tpath = share_sync::ticket_path(share.store.share_dir(), &sp.share_id);
    let mut tmp = tpath.clone();
    tmp.set_extension("tmp");
    if let Err(e) = std::fs::write(&tmp, raw).and_then(|_| std::fs::rename(&tmp, &tpath)) {
        eprintln!("share join: could not persist ticket sidecar: {e}");
    }
    Ok(json!({
        "share_id": sp.share_id,
        "name": sp.name,
        "paper_count": sp.papers.len(),
        "note_count": sp.notes.len(),
        "tag_count": sp.tags.len(),
    }))
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
    use linxiv_core::service::{
        annotation as annotation_svc, note as note_svc, paper as paper_svc,
    };
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
        body: Option<&Value>,
    ) -> Result<Value, ApiError> {
        let segs = split_segments(path);
        let query: HashMap<String, String> = HashMap::new();
        let s: Vec<&str> = segs.iter().map(String::as_str).collect();
        let ctx = ReqCtx {
            method,
            segs: &s,
            query: &query,
            body,
        };
        handle(state, share, &ctx).expect("share arm matched")
    }

    #[tokio::test]
    async fn list_publish_list_envelopes() {
        let (state, pid) = seeded_state();
        let dir = tempfile::tempdir().unwrap();
        let share = ShareState::new(dir.path());

        // Empty before any publish.
        assert_eq!(
            dispatch(&state, &share, "GET", "/api/share/projects", None).unwrap(),
            json!({ "shared_projects": [] })
        );

        // Publish (async arm; nodeless state skips the registry refresh) returns
        // the persisted uuid share_id.
        let resp = publish(&state, &share, &pid.to_string()).await.unwrap();
        let share_id = resp["share_id"].as_str().unwrap().to_string();
        assert_eq!(share_id.len(), 36, "uuid v4 share_id");

        // The summary now lists the published project with sync status fields.
        let listed = dispatch(&state, &share, "GET", "/api/share/projects", None).unwrap();
        let entry = &listed["shared_projects"][0];
        assert_eq!(entry["share_id"], json!(share_id));
        assert_eq!(entry["name"], json!("My Project"));
        assert_eq!(entry["paper_count"], json!(2));
        assert_eq!(entry["note_count"], json!(0));
        assert_eq!(entry["tag_count"], json!(2));
        assert_eq!(entry["paused"], json!(false));
        assert!(
            entry["synced_at"].as_str().is_some(),
            "doc mtime as ISO8601"
        );
        assert!(
            entry.get("project_fk").is_some(),
            "project_fk field present"
        );
    }

    #[tokio::test]
    async fn publish_missing_project_is_404() {
        let (state, _pid) = seeded_state();
        let dir = tempfile::tempdir().unwrap();
        let share = ShareState::new(dir.path());

        let err = publish(&state, &share, "9999").await.unwrap_err();
        assert_eq!(err.status, 404);
    }

    // Needs one bound endpoint to resolve its own loopback addr; relays/discovery
    // are off (bind_offline), so it never contacts an external host — gated like
    // the crate's network tests (multi-thread runtime, no n0 relay).
    #[tokio::test(flavor = "multi_thread")]
    async fn ticket_route_mints_parseable_ticket() {
        let (state, pid) = seeded_state();
        let dir = tempfile::tempdir().unwrap();
        let node = ShareNode::bind_offline(dir.path(), &dir.path().join("p2p"))
            .await
            .unwrap();
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

    // ── W3: import / sync / settings / leave / unpublish ────────────────────

    const ANCHOR: &str = r##"{"v":1,"version":1,"page":1,"color":"#ffd400","quote":"q","rects":[{"x":0,"y":0,"w":0.5,"h":0.1}]}"##;

    const SID: &str = "33333333-3333-4333-8333-333333333333";

    fn empty_state() -> AppState {
        let conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        AppState::from_parts(conn, std::env::temp_dir(), std::env::temp_dir())
    }

    // A remote doc as a reader would have mirrored it after join.
    fn remote_shared(share_id: &str, note_body: &str) -> linxiv_share::SharedProject {
        linxiv_share::SharedProject {
            share_id: share_id.into(),
            name: "Shared P".into(),
            description: "from remote".into(),
            color: Some(0x123456),
            tags: vec!["RL".into()],
            papers: vec![linxiv_share::SharedPaper {
                source_id: "arxiv:9".into(),
                version: 1,
                published: None,
                title: "Remote Paper".into(),
                summary: "s".into(),
                authors: vec!["Zed".into()],
                tags: vec!["remote-tag".into()],
            }],
            notes: vec![linxiv_share::SharedNote {
                uuid: "11111111-1111-4111-8111-111111111111".into(),
                paper_source_id: Some("arxiv:9".into()),
                title: "remote note".into(),
                body: note_body.into(),
                created_at: None,
                updated_at: None,
            }],
            annotations: vec![linxiv_share::SharedAnnotation {
                uuid: "22222222-2222-4222-8222-222222222222".into(),
                paper_source_id: "arxiv:9".into(),
                anchor: ANCHOR.into(),
                comment: "remote highlight".into(),
                created_at: None,
                updated_at: None,
            }],
        }
    }

    #[test]
    fn import_creates_project_papers_notes_tags_canonically() {
        let state = empty_state();
        let dir = tempfile::tempdir().unwrap();
        save(
            &received_dir(dir.path()),
            &remote_shared(SID, "remote body"),
        )
        .unwrap();

        let fk = share_sync::import_received(&state, dir.path(), SID).unwrap();

        state.with_conn(|c| {
            assert_eq!(project_svc::find_by_share_id(c, SID).unwrap(), Some(fk));
            let p = project_svc::get(
                c,
                &project_svc::Project {
                    project_fk: Some(fk),
                },
            )
            .unwrap()
            .unwrap();
            assert_eq!(p.name, "Shared P");
            assert_eq!(p.project_tags, vec!["RL".to_string()]);
            assert_eq!(p.source_fks.len(), 1, "paper linked to project");

            let paper = paper_svc::get(
                c,
                &paper_svc::Paper {
                    source_id: Some("arxiv:9".into()),
                    ..Default::default()
                },
            )
            .unwrap()
            .expect("paper row created");
            assert_eq!(paper.title, "Remote Paper");
            assert_eq!(paper.tags, vec!["remote-tag".to_string()]);

            let notes = note_svc::get_many(
                c,
                &note_svc::Notes {
                    project_fk: Some(fk),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(notes.len(), 1);
            assert_eq!(notes[0].content, "remote body");

            let anns = annotation_svc::get_many(
                c,
                &annotation_svc::Annotations {
                    project_fk: Some(fk),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(anns.len(), 1);
            assert_eq!(anns[0].comment, "remote highlight");
        });
    }

    #[test]
    fn reimport_updates_changed_note_without_duplicating() {
        let state = empty_state();
        let dir = tempfile::tempdir().unwrap();
        let rec = received_dir(dir.path());
        save(&rec, &remote_shared(SID, "v1 body")).unwrap();
        let fk = share_sync::import_received(&state, dir.path(), SID).unwrap();

        // Remote edit arrives: same uuid, new body.
        save(&rec, &remote_shared(SID, "v2 body")).unwrap();
        let fk2 = share_sync::import_received(&state, dir.path(), SID).unwrap();
        assert_eq!(fk, fk2, "re-import links the same project");

        state.with_conn(|c| {
            let notes = note_svc::get_many(
                c,
                &note_svc::Notes {
                    project_fk: Some(fk),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(notes.len(), 1, "matched by uuid, not duplicated");
            assert_eq!(notes[0].content, "v2 body");
            let p = project_svc::get(
                c,
                &project_svc::Project {
                    project_fk: Some(fk),
                },
            )
            .unwrap()
            .unwrap();
            assert_eq!(p.source_fks.len(), 1, "paper not re-linked twice");
        });
    }

    #[tokio::test]
    async fn leave_removes_mirror_ticket_and_settings() {
        let dir = tempfile::tempdir().unwrap();
        let share = ShareState::new(dir.path());
        save(&received_dir(dir.path()), &remote_shared("s-1", "b")).unwrap();
        std::fs::write(share_sync::ticket_path(dir.path(), "s-1"), "tkt").unwrap();
        share_sync::save_settings(dir.path(), "s-1", &share_sync::ShareSettings::default())
            .unwrap();

        leave(&share, "s-1").await.unwrap();

        assert!(!doc_path(&received_dir(dir.path()), "s-1").exists());
        assert!(!share_sync::ticket_path(dir.path(), "s-1").exists());
        assert!(!share_sync::settings_path(dir.path(), "s-1").exists());
        // Second leave: mirror is gone → 404.
        assert_eq!(leave(&share, "s-1").await.unwrap_err().status, 404);
    }

    #[tokio::test]
    async fn settings_roundtrip_and_validation() {
        let state = empty_state();
        let dir = tempfile::tempdir().unwrap();
        let share = ShareState::new(dir.path());
        save(&received_dir(dir.path()), &remote_shared("s-1", "b")).unwrap();

        // Defaults before any write.
        assert_eq!(
            dispatch(&state, &share, "GET", "/api/share/s-1/settings", None).unwrap(),
            json!({ "paused": false, "direction": "two_way" })
        );

        let body = json!({ "paused": true, "direction": "shared_to_local" });
        put_settings(&share, "s-1", Some(&body)).await.unwrap();
        assert_eq!(
            dispatch(&state, &share, "GET", "/api/share/s-1/settings", None).unwrap(),
            json!({ "paused": true, "direction": "shared_to_local" })
        );

        let bad = json!({ "direction": "upstream" });
        let err = put_settings(&share, "s-1", Some(&bad)).await.unwrap_err();
        assert_eq!(err.status, 422);
    }

    // Route-level revocation: unpublish deletes the doc file, so a held ticket's
    // fetch is refused (existence-based access check) — offline loopback only.
    #[tokio::test(flavor = "multi_thread")]
    async fn unpublish_then_fetch_is_not_found() {
        let (state, pid) = seeded_state();
        let a_dir = tempfile::tempdir().unwrap();
        let node = ShareNode::bind_offline(a_dir.path(), &a_dir.path().join("p2p"))
            .await
            .unwrap();
        let share = ShareState::with_node(a_dir.path(), node);

        let resp = ticket(&state, &share, &pid.to_string()).await.unwrap();
        let parsed: ShareTicket = resp["ticket"].as_str().unwrap().parse().unwrap();
        let share_id = resp["share_id"].as_str().unwrap().to_string();

        let resp = unpublish(&share, &share_id).await.unwrap();
        assert_eq!(resp["unpublished"], json!(true));
        // Unpublishing twice is a 404 (doc already gone).
        assert_eq!(unpublish(&share, &share_id).await.unwrap_err().status, 404);

        let b_dir = tempfile::tempdir().unwrap();
        let b = ShareNode::bind_offline(b_dir.path(), &b_dir.path().join("p2p"))
            .await
            .unwrap();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            b.fetch(&parsed, b_dir.path()),
        )
        .await
        .expect("loopback fetch should not hang");
        assert!(
            matches!(result, Err(ShareError::NotFound(_))),
            "unpublished share must refuse fetch, got {result:?}"
        );

        b.shutdown().await.unwrap();
        share.shutdown().await.unwrap();
    }
}
