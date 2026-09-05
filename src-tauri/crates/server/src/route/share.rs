//! `/api/share` routes — Phase-0 quarantined CRDT "shared projects". A second
//! front door beside `api`: `share_api` resolves `ShareState` alongside the
//! canonical `AppState`. Publishing only READS `papers.db` (through linxiv-share's
//! read-only `publish`); the CRDT docs live under the injected share directory.
//!
//! `route()` and its callers (the dev_server bin, the linxiv:// protocol handler)
//! are unchanged — this dispatcher is invoked only via the `share_api` command.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;

/// Cap on a single network op (mint ticket / fetch); past it the request returns
/// 504 and releases the node lock so shutdown is not blocked.
pub(crate) const SHARE_NET_TIMEOUT: Duration = Duration::from_secs(30);

use linxiv_core::config;
use linxiv_core::service::paper::{self as paper_svc, pdf_on_disk_name};
use linxiv_core::service::paper_import;
use linxiv_core::service::project as project_svc;
use linxiv_share::{
    build_shared_project, doc_path, e2ee_dir, e2ee_received_dir, member_id_from_hex, member_id_hex,
    received_dir, save, valid_share_id, AutoCommit, CustomRelay, ProjectInvite, Role, ShareError,
    ShareNode, ShareStore, ShareTicket, SharedProject,
};

use crate::p2p_config::{self, RelaySetting};
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
    // Flipped once, by whichever of {startup, a later `rebind`} first finds a
    // bound node — guards against spawning the background interval-sync loop
    // twice (or never, if startup found no node but a reconnect later does).
    sync_started: AtomicBool,
    // Remote Query Mode (headless only): re-applied to every fresh node, so
    // a relay reconnect's rebind doesn't silently drop the api handler.
    api_installer: std::sync::Mutex<Option<ApiInstallFn>>,
}

/// Installs the `linxiv-api/1` handler on a freshly bound node (headless bin;
/// the desktop app never sets one).
pub type ApiInstallFn = Arc<dyn Fn(&ShareNode) + Send + Sync>;

impl ShareState {
    /// Store-only state (no network node). Used by the Phase-0 sync unit tests.
    pub fn new(share_dir: impl Into<PathBuf>) -> Self {
        Self {
            store: ShareStore::new(share_dir),
            node: Mutex::new(None),
            write_locks: Mutex::new(HashMap::new()),
            sync_started: AtomicBool::new(false),
            api_installer: std::sync::Mutex::new(None),
        }
    }

    /// Full state with a live iroh node serving the same share directory.
    pub fn with_node(share_dir: impl Into<PathBuf>, node: ShareNode) -> Self {
        Self {
            store: ShareStore::new(share_dir),
            node: Mutex::new(Some(Arc::new(node))),
            write_locks: Mutex::new(HashMap::new()),
            sync_started: AtomicBool::new(false),
            api_installer: std::sync::Mutex::new(None),
        }
    }

    /// Share directory backing this state (docs + settings/ticket sidecars).
    pub fn share_dir(&self) -> &Path {
        self.store.share_dir()
    }

    /// Clone the live node out from under the lock (`None` while store-only).
    /// `pub`: the app's remote_backend dials linxiv-api/1 over this node.
    pub async fn node(&self) -> Option<Arc<ShareNode>> {
        self.node.lock().await.clone()
    }

    /// This node's iroh endpoint id (`None` while unbound). Status reporting.
    pub async fn endpoint_id(&self) -> Option<String> {
        self.node().await.map(|n| n.endpoint_id())
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

    /// Marks the background interval-sync loop as started. Returns `true`
    /// only for the caller that flips it, so exactly one loop ever runs
    /// regardless of whether the node came up at startup or via a later
    /// [`Self::rebind`].
    pub fn mark_sync_started(&self) -> bool {
        self.sync_started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// Tears the current node down (if any) and binds a fresh one with
    /// `relay`, swapping it into place — the "Save & Reconnect" relay flow,
    /// so a relay change doesn't need a full app restart. A bind failure
    /// leaves the node unbound (sharing disabled) rather than retrying the
    /// old config: silently keeping the previous relay after the user asked
    /// to switch would be its own kind of unwanted fallback.
    pub async fn rebind(
        &self,
        p2p_dir: &Path,
        dek: Option<[u8; 32]>,
        relay: Option<CustomRelay>,
    ) -> Result<(), ShareError> {
        let mut guard = self.node.lock().await;
        if let Some(old) = guard.take() {
            old.shutdown().await?;
        }
        let fresh = ShareNode::bind_with_dek(self.store.share_dir(), p2p_dir, dek, relay).await?;
        if let Some(install) = self.api_installer.lock().unwrap().clone() {
            install(&fresh);
        }
        *guard = Some(Arc::new(fresh));
        Ok(())
    }

    /// Registers the Remote Query Mode installer and applies it to the
    /// current node (if bound). `rebind` re-applies it to every fresh node.
    pub async fn install_api(&self, install: ApiInstallFn) {
        *self.api_installer.lock().unwrap() = Some(install.clone());
        if let Some(node) = self.node().await {
            install(&node);
        }
    }
}

/// Resolve relay settings + bind the startup share node — shared by the
/// packaged app's setup and the headless bin. A bind failure or a
/// required-but-missing relay degrades to a store-only state (sharing
/// disabled) with a warning instead of failing startup. `dek` is resolved by
/// the caller: keychain access is sync and must not run inside an async
/// context. Returns `(state, node_bound)`.
pub async fn startup_share_state(dek: Option<[u8; 32]>) -> std::io::Result<(ShareState, bool)> {
    let share_dir = config::data_dir().join("share");
    std::fs::create_dir_all(&share_dir)?;
    // Persisted device key lives beside (not inside) the served share dir.
    let p2p_dir = config::data_dir().join("p2p");
    Ok(match p2p_config::relay_setting() {
        RelaySetting::RequireCustomButMissing => {
            eprintln!(
                "warning: \"only use this relay\" is on but no valid custom relay is configured; refusing to fall back to the public n0 relay, sharing disabled"
            );
            (ShareState::new(share_dir), false)
        }
        setting => {
            let relay = match setting {
                RelaySetting::Custom(relay) => Some(relay),
                _ => None,
            };
            match ShareNode::bind_with_dek(share_dir.clone(), &p2p_dir, dek, relay).await {
                Ok(node) => (ShareState::with_node(share_dir, node), true),
                Err(e) => {
                    eprintln!(
                        "warning: share node bind failed, sharing (plain and e2ee) and background sync disabled: {e}"
                    );
                    (ShareState::new(share_dir), false)
                }
            }
        }
    })
}

impl From<ShareError> for ApiError {
    fn from(e: ShareError) -> Self {
        let status = match &e {
            ShareError::NotFound(_) => 404,
            ShareError::Core(c) => c.http_status(),
            ShareError::Transport(_) => 502,
            ShareError::TooLarge(_) => 413,
            ShareError::RoleConflict | ShareError::LastReader => 409,
            ShareError::Io(_) | ShareError::Crdt(_) => 500,
        };
        ApiError::new(status, e.to_string())
    }
}

/// Tauri-free `/api/share/*` dispatcher: the app's `share_api` command and the
/// headless bin both route through here. `spawn_sync` starts the background
/// interval-sync loop when the relay-reconnect arm brings the first node up —
/// each caller owns how that task is spawned.
pub async fn dispatch(
    state: &AppState,
    share: &ShareState,
    spawn_sync: &(dyn Fn() + Sync),
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
            return ticket(state, share, id).await
        }
        ("POST", ["api", "share", "join"]) => return join(share, ctx.body).await,
        ("POST", ["api", "share", "project", id, "publish"]) => {
            return publish(state, share, id).await
        }
        ("POST", ["api", "share", id, "unpublish"]) => return unpublish(share, id).await,
        ("POST", ["api", "share", "received", id, "import"]) => {
            if !valid_share_id(id) {
                return Err(ApiError::new(404, format!("share {id:?} not found")));
            }
            let _lock = share.lock_writes(id).await;
            return share_sync::import_received(state, share.share_dir(), id)
                .map(|fk| json!({ "project_fk": fk }));
        }
        ("POST", ["api", "share", "received", id, "leave"]) => return leave(share, id).await,
        ("POST", ["api", "share", "received", id, "unlink"]) => {
            return unlink(state, share, id).await
        }
        ("POST", ["api", "share", id, "sync"]) => {
            return share_sync::sync_share(state, share, id).await
        }
        ("PUT" | "POST", ["api", "share", id, "settings"]) => {
            return put_settings(share, id, ctx.body).await
        }
        ("GET", ["api", "share", "member_code"]) => return member_code(share).await,
        ("POST", ["api", "share", "relay", "reconnect"]) => {
            return reconnect_relay(spawn_sync, share).await
        }
        // Shadows the sync arm in `handle` so the live app gets role-stamped
        // summaries; the store-only tests keep dispatching through `handle`.
        ("GET", ["api", "share", "received"]) => {
            return list_received_with_role(state, share).await
        }
        ("POST", ["api", "share", "project", id, "publish_secure"]) => {
            return publish_secure(state, share, id).await
        }
        ("POST", ["api", "share", id, "invite"]) => {
            return invite(state, share, id, ctx.body).await
        }
        ("GET", ["api", "share", id, "members"]) => return members(share, id).await,
        ("POST", ["api", "share", id, "member", mid, "role"]) => {
            return set_member_role(state, share, id, mid, ctx.body).await
        }
        ("POST", ["api", "share", id, "revoke"]) => {
            return revoke_member(share, id, ctx.body).await
        }
        ("POST", ["api", "share", id, "member", mid, "remove"]) => {
            return remove_member(share, id, mid).await
        }
        ("POST", ["api", "share", id, "rekey"]) => return rekey(state, share, id).await,
        ("POST", ["api", "share", id, "pdf"]) => {
            return shared_pdf(state, share, id, ctx.body).await
        }
        _ => {}
    }
    handle(state, share, &ctx).unwrap_or_else(|| Err(ApiError::not_routed()))
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
/// `pub` so the headless bin's status aggregate reuses it.
pub fn list_shared(state: &AppState, share: &ShareState) -> Result<Value, ApiError> {
    let dir = share.store.share_dir();
    let mut out = Vec::new();
    for s in share.store.list_shared()? {
        let mut v = summary_json(&s, &doc_path(dir, &s.share_id), dir);
        let fk = state.with_conn(|c| project_svc::find_by_share_id(c, &s.share_id))?;
        v["project_fk"] = json!(fk);
        out.push(v);
    }
    for s in ShareNode::list_e2ee(dir)? {
        let mut v = summary_json(&s, &doc_path(&e2ee_dir(dir), &s.share_id), dir);
        let fk = state.with_conn(|c| project_svc::find_by_share_id(c, &s.share_id))?;
        v["project_fk"] = json!(fk);
        v["e2ee"] = json!(true);
        v["member_count"] = json!(live_member_count(dir, &s.share_id));
        out.push(v);
    }
    Ok(json!({ "shared_projects": out }))
}

/// `GET /api/share/received` — summaries of every mirror materialized by `join`,
/// each carrying the `project_fk` of the linked local project (null pre-import).
/// `pub` so the headless bin's status aggregate reuses it (no role queries).
pub fn list_received(state: &AppState, share: &ShareState) -> Result<Value, ApiError> {
    let dir = share.store.share_dir();
    let rec = received_dir(dir);
    let mut out = Vec::new();
    for s in linxiv_share::ShareNode::list_received(dir)? {
        let mut v = summary_json(&s, &doc_path(&rec, &s.share_id), dir);
        let fk = state.with_conn(|c| project_svc::find_by_share_id(c, &s.share_id))?;
        v["project_fk"] = json!(fk);
        out.push(v);
    }
    for s in ShareNode::list_e2ee_received(dir)? {
        let mut v = summary_json(&s, &doc_path(&e2ee_received_dir(dir), &s.share_id), dir);
        let fk = state.with_conn(|c| project_svc::find_by_share_id(c, &s.share_id))?;
        v["project_fk"] = json!(fk);
        v["e2ee"] = json!(true);
        out.push(v);
    }
    out.extend(pending_received(dir, &out));
    Ok(json!({ "received": out }))
}

/// Mirrors under `e2ee/received` whose doc holds no content yet: `accept_invite`
/// writes an empty placeholder when the host is unreachable, and an empty doc
/// never hydrates, so the listing above drops it and the join vanishes from the
/// UI. Surfaced as `pending` so a user can retry the sync (or leave) by hand.
fn pending_received(dir: &Path, listed: &[Value]) -> Vec<Value> {
    let synced: Vec<&str> = listed
        .iter()
        .filter_map(|v| v["share_id"].as_str())
        .collect();
    share_sync::doc_ids(&e2ee_received_dir(dir))
        .into_iter()
        .filter(|id| !synced.contains(&id.as_str()))
        .map(|id| {
            let paused = share_sync::load_settings(dir, &id).paused;
            json!({
                "share_id": id,
                "name": "",
                "paper_count": 0,
                "note_count": 0,
                "tag_count": 0,
                // The placeholder's mtime is the join, not a sync — report none.
                "synced_at": Value::Null,
                "paused": paused,
                "project_fk": Value::Null,
                "e2ee": true,
                "pending": true,
            })
        })
        .collect()
}

/// `list_received` plus the reader's own capability (spec §7): each e2ee
/// entry gets `role` ("viewer" | "editor") from a live `query_role` against
/// this device's member id. Absent when the node is offline or on plain
/// mirrors — the GUI treats an unknown role as editable (no regression);
/// enforcement is server+crypto, this field is UX only.
async fn list_received_with_role(state: &AppState, share: &ShareState) -> Result<Value, ApiError> {
    let mut v = list_received(state, share)?;
    let Some(node) = share.node().await else {
        return Ok(v);
    };
    let Ok(me) = node.self_member_id() else {
        return Ok(v);
    };
    let Some(list) = v.get_mut("received").and_then(Value::as_array_mut) else {
        return Ok(v);
    };
    // Pending mirrors are skipped: there is no content for a role to gate, and
    // one unanswered query per pending share would stall the whole listing.
    let targets: Vec<(usize, String)> = list
        .iter()
        .enumerate()
        .filter(|(_, e)| e["e2ee"] == json!(true) && e["pending"] != json!(true))
        .filter_map(|(i, e)| e["share_id"].as_str().map(|s| (i, s.to_string())))
        .collect();
    // Queries run concurrently: each keeps its own timeout budget, but a slow
    // relay no longer stalls the listing by (entries × budget).
    let roles = futures_util::future::join_all(
        targets
            .iter()
            .map(|(_, sid)| e2ee_timeout(node.query_role(sid, me), "role query")),
    )
    .await;
    for ((i, _), res) in targets.iter().zip(roles) {
        // A failed or empty query leaves `role` unset (degrades editable).
        if let Ok(Some(role)) = res {
            list[*i]["role"] = match role {
                Role::Read => json!("viewer"),
                Role::Edit => json!("editor"),
                Role::Admin => json!("hoster"),
                Role::Relay => continue,
            };
        }
    }
    Ok(v)
}

/// A doc file for `id` in any role (plain/e2ee, hosted/received).
fn any_doc_exists(dir: &Path, id: &str) -> bool {
    doc_path(dir, id).is_file()
        || doc_path(&received_dir(dir), id).is_file()
        || doc_path(&e2ee_dir(dir), id).is_file()
        || doc_path(&e2ee_received_dir(dir), id).is_file()
}

/// `GET /api/share/{id}/settings` — the per-share sidecar (defaults if unset).
fn get_settings(share: &ShareState, id: &str) -> Result<Value, ApiError> {
    if !valid_share_id(id) {
        return Err(ApiError::new(404, format!("share {id:?} not found")));
    }
    let dir = share.share_dir();
    if !any_doc_exists(dir, id) {
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
    if !any_doc_exists(dir, id) {
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

// ── e2ee members sidecar (`share_dir/members/<id>.json`) ────────────────────

/// One invited device on a hoster-owned e2ee share. The sidecar IS the members
/// list — the wrapper has no listing API; `query_role` is the live truth-check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct MemberEntry {
    pub member_id_hex: String,
    #[serde(default)]
    pub name: Option<String>,
    /// "hoster" | "editor" | "viewer"
    pub role: String,
    pub invited_at: String,
    #[serde(default)]
    pub revoked: bool,
    /// The last invite string minted for this member, kept so the host can
    /// re-send it without asking for the member code again. Absent on
    /// pre-upgrade sidecars, and cleared whenever the grant changes (revoke /
    /// role change) since the string that survives is then stale.
    // ponytail: a bearer capability at rest in the share dir, beside the doc
    // and key store it grants against; upgrade: store it in the key store.
    #[serde(default)]
    pub invite: Option<String>,
}

/// Non-revoked, non-hoster sidecar rows — "how many devices this share is
/// currently granted to", for the hoster sync line.
pub(crate) fn live_member_count(share_dir: &Path, share_id: &str) -> usize {
    load_members(share_dir, share_id)
        .iter()
        .filter(|m| !m.revoked && m.role != "hoster")
        .count()
}

fn members_path(share_dir: &Path, share_id: &str) -> PathBuf {
    share_dir.join("members").join(format!("{share_id}.json"))
}

/// Missing or corrupt sidecar → empty list.
pub(crate) fn load_members(share_dir: &Path, share_id: &str) -> Vec<MemberEntry> {
    std::fs::read(members_path(share_dir, share_id))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_members(share_dir: &Path, share_id: &str, list: &[MemberEntry]) -> std::io::Result<()> {
    let path = members_path(share_dir, share_id);
    std::fs::create_dir_all(path.parent().expect("members path has a parent"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(list).expect("members serialize"))?;
    std::fs::rename(&tmp, &path)
}

/// 404 unless `share_id` is a hoster-owned e2ee doc.
fn ensure_e2ee_hosted(share_dir: &Path, share_id: &str) -> Result<(), ApiError> {
    if !valid_share_id(share_id) || !doc_path(&e2ee_dir(share_dir), share_id).is_file() {
        return Err(ApiError::new(
            404,
            format!("e2ee share {share_id:?} not found"),
        ));
    }
    Ok(())
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
/// `<id>.automerge.unpublished` and delete its settings sidecar. An e2ee doc
/// additionally revokes every active member first, which stops the beelay node
/// serving it.
async fn unpublish(share: &ShareState, id: &str) -> Result<Value, ApiError> {
    if !valid_share_id(id) {
        return Err(ApiError::new(404, format!("share {id:?} not found")));
    }
    let dir = share.share_dir();
    let _lock = share.lock_writes(id).await;
    let doc = doc_path(dir, id);
    if doc.is_file() {
        std::fs::rename(&doc, unpublished_path(&doc))
            .map_err(|e| ApiError::new(500, format!("could not unpublish: {e}")))?;
        let _ = std::fs::remove_file(share_sync::settings_path(dir, id));
        return Ok(json!({ "unpublished": true, "share_id": id }));
    }
    let e2ee_doc = doc_path(&e2ee_dir(dir), id);
    if !e2ee_doc.is_file() {
        return Err(ApiError::new(404, format!("share {id:?} not found")));
    }
    // Revocation runs against the live node (content lives in beelay state).
    let node = live_node(share).await?;
    let mut list = load_members(dir, id);
    let mut failed = Vec::new();
    for m in list.iter_mut().filter(|m| !m.revoked && m.role != "hoster") {
        let Some(mid) = member_id_from_hex(&m.member_id_hex) else {
            eprintln!(
                "share {id}: marking malformed member id {} revoked",
                m.member_id_hex
            );
            m.revoked = true;
            continue;
        };
        // query_role == None: keyhive already dropped them; just mark the sidecar.
        if matches!(
            e2ee_timeout(node.query_role(id, mid), "member query").await,
            Ok(None)
        ) {
            m.revoked = true;
            continue;
        }
        match e2ee_timeout(node.revoke(id, mid), "revoke").await {
            Ok(_) => m.revoked = true,
            Err(e) => failed.push(format!("{}: {}", m.member_id_hex, e.detail)),
        }
    }
    if let Err(e) = save_members(dir, id, &list) {
        eprintln!("share {id}: could not persist members sidecar: {e}");
    }
    if !failed.is_empty() {
        return Err(ApiError::new(
            502,
            format!("unpublish aborted, could not revoke: {}", failed.join("; ")),
        ));
    }
    std::fs::rename(&e2ee_doc, unpublished_path(&e2ee_doc))
        .map_err(|e| ApiError::new(500, format!("could not unpublish: {e}")))?;
    let _ = std::fs::remove_file(share_sync::settings_path(dir, id));
    let _ = std::fs::remove_file(members_path(dir, id));
    Ok(json!({ "unpublished": true, "share_id": id, "e2ee": true }))
}

/// `POST /api/share/received/{id}/leave` — delete the mirror + ticket +
/// settings, and drop the beelay registration behind an e2ee mirror so a later
/// rejoin adopts from scratch. `forgotten: false` means the p2p node was down,
/// so the registration survived: rejoining would reuse the old document.
async fn leave(share: &ShareState, id: &str) -> Result<Value, ApiError> {
    if !valid_share_id(id) {
        return Err(ApiError::new(404, format!("share {id:?} not found")));
    }
    let dir = share.share_dir();
    let _lock = share.lock_writes(id).await;
    let mirror = doc_path(&received_dir(dir), id);
    let e2ee_mirror = doc_path(&e2ee_received_dir(dir), id);
    let was_e2ee = e2ee_mirror.is_file();
    let target = [&mirror, &e2ee_mirror].into_iter().find(|p| p.is_file());
    let Some(target) = target else {
        return Err(ApiError::new(
            404,
            format!("received share {id:?} not found"),
        ));
    };
    // Beelay first: deleting the mirror while the registration survives is the
    // half-state that makes a rejoin silently reuse the old doc.
    let mut forgotten = !was_e2ee;
    if was_e2ee {
        match share.node().await {
            Some(node) => {
                e2ee_timeout(node.forget_e2ee(id), "leave share").await?;
                forgotten = true;
            }
            // Not fatal: the user asked to leave, and the files below are what
            // the interval loop reads. The response says the undo is partial.
            None => eprintln!("share {id}: leaving with p2p offline; beelay entry survives"),
        }
    }
    std::fs::remove_file(target)
        .map_err(|e| ApiError::new(500, format!("could not leave share: {e}")))?;
    let _ = std::fs::remove_file(share_sync::ticket_path(dir, id));
    let _ = std::fs::remove_file(share_sync::settings_path(dir, id));
    Ok(json!({ "left": true, "forgotten": forgotten }))
}

/// `POST /api/share/received/{id}/unlink` — detach the linked local project
/// from a received share. Membership, mirror, and the project all stay; the
/// interval sync keeps refreshing the mirror but stops importing (its import
/// legs are gated on `find_by_share_id`). Re-importing creates a fresh link.
async fn unlink(state: &AppState, share: &ShareState, id: &str) -> Result<Value, ApiError> {
    if !valid_share_id(id) {
        return Err(ApiError::new(404, format!("share {id:?} not found")));
    }
    let dir = share.share_dir();
    // Received mirrors only: clearing a hoster project's SHARE_ID would drop
    // its publish identity.
    if !doc_path(&received_dir(dir), id).is_file()
        && !doc_path(&e2ee_received_dir(dir), id).is_file()
    {
        return Err(ApiError::new(
            404,
            format!("received share {id:?} not found"),
        ));
    }
    let _lock = share.lock_writes(id).await;
    let unlinked = state.with_conn(|c| project_svc::release_share_id(c, id))?;
    Ok(json!({ "unlinked": unlinked }))
}

/// `GET /api/share/received/{id}` — the full subgraph of one received mirror
/// (plain, falling back to the e2ee mirror of the same id).
fn get_received(share: &ShareState, id: &str) -> Result<Value, ApiError> {
    let dir = share.store.share_dir();
    let sp = match linxiv_share::ShareNode::received(dir, id) {
        Err(ShareError::NotFound(_)) => linxiv_share::ShareNode::e2ee_received(dir, id)?,
        other => other?,
    };
    Ok(json!({
        "share_id": sp.share_id,
        "name": sp.name,
        "description": sp.description,
        "color": sp.color,
        "tags": sp.tags,
        "papers": sp.papers.iter().map(|p| p.to_summary_value()).collect::<Vec<_>>(),
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
    let (sp, doc, _lock) = publish_plain(state, share, id).await?;
    if let Some(node) = share.node().await {
        node.register_doc(&sp.share_id, doc)?;
    }
    Ok(json!({ "share_id": sp.share_id }))
}

/// Snapshot the project, refuse ids owned by a received or e2ee share, and save
/// the plain doc under the share's write lock — the shared half of `publish` and
/// `ticket`. The returned lock guard is held until the caller drops it.
async fn publish_plain(
    state: &AppState,
    share: &ShareState,
    id: &str,
) -> Result<(SharedProject, AutoCommit, tokio::sync::OwnedMutexGuard<()>), ApiError> {
    let project_id = path_i64(id)?;
    let sp = state.with_conn(|conn| build_shared_project(conn, project_id))?;
    let dir = share.store.share_dir();
    if doc_path(&received_dir(dir), &sp.share_id).is_file() {
        return Err(ApiError::new(
            409,
            "project is linked to a received share; leave the share before publishing",
        ));
    }
    if doc_path(&e2ee_dir(dir), &sp.share_id).is_file()
        || doc_path(&e2ee_received_dir(dir), &sp.share_id).is_file()
    {
        return Err(ApiError::new(
            409,
            "project is published as an encrypted share; unpublish it before publishing plain",
        ));
    }
    let lock = share.lock_writes(&sp.share_id).await;
    restore_unpublished(dir, &sp.share_id);
    let doc = save(dir, &sp)?;
    Ok((sp, doc, lock))
}

// Clone the node Arc out from under the lock, then release it: the 30s network
// op must not hold the guard `shutdown()` also needs.
async fn live_node(share: &ShareState) -> Result<Arc<ShareNode>, ApiError> {
    share
        .node()
        .await
        .ok_or_else(|| ApiError::new(503, "share transport not initialized"))
}

/// `POST /api/share/project/{id}/ticket` — ensure the project is published
/// (Phase-0 publish, read-only over the canonical connection), then mint a
/// pasteable ticket carrying the sender's address + share id; access is gated
/// by whether that id is currently published, not a per-recipient secret.
async fn ticket(state: &AppState, share: &ShareState, id: &str) -> Result<Value, ApiError> {
    // `_` (not `_doc`): drop the saved doc now rather than holding it across the
    // network call — `ticket` re-reads it from disk anyway.
    let (sp, _, _lock) = publish_plain(state, share, id).await?;

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
    let ticket: ShareTicket = match raw.parse() {
        Ok(t) => t,
        // Not a plain ticket — maybe an e2ee invite.
        Err(ticket_err) => return join_invite(share, raw, ticket_err).await,
    };

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

/// `join` fall-through for e2ee invites: accept the invite (adopts + one sync +
/// mirror under `e2ee/received/`) and return the same joined-summary shape. No
/// ticket sidecar — the host address lives in beelay state.
async fn join_invite(
    share: &ShareState,
    raw: &str,
    ticket_err: impl std::fmt::Display,
) -> Result<Value, ApiError> {
    let invite: ProjectInvite = raw.parse().map_err(|invite_err| {
        ApiError::new(
            400,
            format!("not a share ticket or invite: {ticket_err}; {invite_err}"),
        )
    })?;
    if !valid_share_id(invite.project_id()) {
        return Err(ApiError::new(400, "invite has a malformed project id"));
    }
    let node = live_node(share).await?;
    let _lock = share.lock_writes(invite.project_id()).await;
    let dir = share.share_dir();
    let iid = invite.project_id();
    if doc_path(dir, iid).is_file()
        || doc_path(&e2ee_dir(dir), iid).is_file()
        || doc_path(&received_dir(dir), iid).is_file()
        || doc_path(&e2ee_received_dir(dir), iid).is_file()
    {
        return Err(ApiError::new(
            409,
            "share id is already published or mirrored here; unpublish or leave it first",
        ));
    }
    let accepted = e2ee_timeout(node.accept_invite(raw), "share join").await?;
    // Host asleep: the invite is saved and the share syncs on a later pass, so
    // this is a success with nothing to summarize yet — no mirror to hydrate
    // (it is an empty placeholder) and no counts to report.
    if accepted.pending {
        return Ok(json!({
            "share_id": accepted.share_id,
            "e2ee": true,
            "pending": true,
            "reason": "host unreachable; the invite is saved and will finish syncing when the host is online",
        }));
    }
    let share_id = accepted.share_id;
    let sp = match ShareNode::e2ee_received(share.share_dir(), &share_id) {
        Ok(sp) => sp,
        // Dialled the host, but nothing decrypted into the mirror yet (no key
        // for our epoch). Same shape as the asleep-host case: adopted, pending.
        Err(ShareError::NotFound(_)) => {
            return Ok(json!({
                "share_id": share_id,
                "e2ee": true,
                "pending": true,
                "reason": "joined, but no content has decrypted yet; it will finish syncing shortly",
            }))
        }
        Err(e) => return Err(e.into()),
    };
    Ok(json!({
        "share_id": sp.share_id,
        "name": sp.name,
        "paper_count": sp.papers.len(),
        "note_count": sp.notes.len(),
        "tag_count": sp.tags.len(),
        "e2ee": true,
    }))
}

/// Map a `fetch` failure to a status: a refused/unknown capability is a 404 (the
/// peer answered, the doc just isn't served to us); any other failure during the
/// live dial is an upstream/transport fault, surfaced as 502 — never a blanket 500.
fn fetch_error(e: ShareError) -> ApiError {
    match e {
        ShareError::NotFound(_) => ApiError::new(404, e.to_string()),
        // Typed capability conflicts keep their 409 through the live-dial path.
        ShareError::RoleConflict | ShareError::LastReader => ApiError::new(409, e.to_string()),
        _ => ApiError::new(502, e.to_string()),
    }
}

// ── W4: e2ee arms ────────────────────────────────────────────────────────────

/// Keyhive/BeeKEM ops run slower than plain sync; e2ee arms double the budget.
async fn e2ee_timeout<T>(
    fut: impl std::future::Future<Output = Result<T, ShareError>>,
    what: &str,
) -> Result<T, ApiError> {
    tokio::time::timeout(SHARE_NET_TIMEOUT * 2, fut)
        .await
        .map_err(|_| ApiError::new(504, format!("{what} timed out")))?
        .map_err(fetch_error)
}

/// `GET /api/share/member_code` — this device's pasteable membership code.
async fn member_code(share: &ShareState) -> Result<Value, ApiError> {
    let node = live_node(share).await?;
    let code = e2ee_timeout(node.member_code(), "member code").await?;
    Ok(json!({ "code": code }))
}

/// `POST /api/share/relay/reconnect` — rebind the p2p node against whatever
/// is currently saved under Settings → Sharing, without an app restart. Save
/// the relay settings first (`PATCH /api/settings`), then call this.
async fn reconnect_relay(
    spawn_sync: &(dyn Fn() + Sync),
    share: &ShareState,
) -> Result<Value, ApiError> {
    let p2p_dir = config::data_dir().join("p2p");
    match p2p_config::relay_setting() {
        RelaySetting::RequireCustomButMissing => {
            share.shutdown().await?;
            return Err(ApiError::new(
                400,
                "\"Only use this relay\" is on but no valid relay is configured; refusing to fall back to the public n0 relay",
            ));
        }
        setting => {
            let relay = match setting {
                RelaySetting::Custom(relay) => Some(relay),
                _ => None,
            };
            // Keychain access is sync (the Linux backend block_ons its own
            // runtime and panics on a tokio worker thread) — resolve the DEK
            // off the async worker. Join failure degrades to no DEK, same as
            // an unavailable keychain.
            let dek = tokio::task::spawn_blocking(p2p_config::p2p_dek)
                .await
                .unwrap_or(None);
            share.rebind(&p2p_dir, dek, relay).await?;
        }
    }
    if share.mark_sync_started() {
        spawn_sync();
    }
    Ok(json!({ "ok": true }))
}

/// `POST /api/share/project/{id}/publish_secure` — snapshot a canonical project
/// into an e2ee share (doc under `share_dir/e2ee`, beelay-registered), sharing
/// PDF blobs for papers with a local file, and seed the members sidecar with
/// this device as hoster.
async fn publish_secure(state: &AppState, share: &ShareState, id: &str) -> Result<Value, ApiError> {
    let project_id = path_i64(id)?;
    let mut sp = state.with_conn(|conn| build_shared_project(conn, project_id))?;
    let dir = share.share_dir().to_path_buf();
    if doc_path(&received_dir(&dir), &sp.share_id).is_file()
        || doc_path(&e2ee_received_dir(&dir), &sp.share_id).is_file()
    {
        return Err(ApiError::new(
            409,
            "project is linked to a received share; leave the share before publishing",
        ));
    }
    if doc_path(&dir, &sp.share_id).is_file() {
        return Err(ApiError::new(
            409,
            "project is published as a plain share; unpublish it before publishing encrypted",
        ));
    }
    let node = live_node(share).await?;
    let _lock = share.lock_writes(&sp.share_id).await;
    restore_unpublished(&e2ee_dir(&dir), &sp.share_id);
    if doc_path(&e2ee_dir(&dir), &sp.share_id).is_file() {
        // Republish: doc still on disk, so populate reads its tickets before publish overwrites it.
        share_sync::populate_pdf_blobs(state, &node, &dir, &mut sp, false).await?;
        e2ee_timeout(node.publish_secure(&sp), "secure publish").await?;
    } else {
        // Brand-new share: the beelay project must exist before blob storage
        // can succeed. A populate failure after this point still errors the
        // request; the share exists and a retry recovers via the republish arm.
        e2ee_timeout(node.publish_secure(&sp), "secure publish").await?;
        share_sync::populate_pdf_blobs(state, &node, &dir, &mut sp, false).await?;
        if sp.papers.iter().any(|p| p.pdf_blob.is_some()) {
            e2ee_timeout(node.publish_secure(&sp), "secure publish").await?;
        }
    }
    let mut list = load_members(&dir, &sp.share_id);
    if !list.iter().any(|m| m.role == "hoster") {
        list.push(MemberEntry {
            member_id_hex: node
                .self_member_id()
                .map(|m| member_id_hex(&m))
                .unwrap_or_default(),
            name: None,
            role: "hoster".into(),
            invited_at: chrono::Utc::now().to_rfc3339(),
            revoked: false,
            invite: None,
        });
        if let Err(e) = save_members(&dir, &sp.share_id, &list) {
            eprintln!(
                "share {}: could not persist members sidecar: {e}",
                sp.share_id
            );
        }
    }
    Ok(json!({ "share_id": sp.share_id, "e2ee": true }))
}

/// `POST /api/share/{id}/invite {member_code, role: "editor"|"viewer", name?}`
/// — grant a device access to a hoster-owned e2ee share and mint its invite.
async fn invite(
    state: &AppState,
    share: &ShareState,
    id: &str,
    body: Option<&Value>,
) -> Result<Value, ApiError> {
    let dir = share.share_dir().to_path_buf();
    ensure_e2ee_hosted(&dir, id)?;
    let code = body
        .and_then(|b| b.get("member_code"))
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::new(422, "missing `member_code` in body"))?;
    if code.is_empty()
        || code.len() % 2 != 0
        || !code.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(ApiError::new(422, "`member_code` must be lowercase hex"));
    }
    let role_s = body
        .and_then(|b| b.get("role"))
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::new(422, "missing `role` in body"))?;
    let role = match role_s {
        "editor" => Role::Edit,
        "viewer" => Role::Read,
        _ => return Err(ApiError::new(422, "role must be \"editor\" or \"viewer\"")),
    };
    let name = body
        .and_then(|b| b.get("name"))
        .and_then(Value::as_str)
        .map(String::from);
    let node = live_node(share).await?;
    let _lock = share.lock_writes(id).await;
    // A concurrent unpublish may have parked the doc between the entry check and the lock.
    ensure_e2ee_hosted(&dir, id)?;
    // A typed ShareError::RoleConflict surfaces as 409 via fetch_error.
    let (member, invite) = e2ee_timeout(node.invite_member(id, code, role), "invite").await?;
    // Keyhive accepted the grant, so a sidecar entry disagreeing on role is
    // stale — overwritten below, never a post-grant 409.
    let hex = member_id_hex(&member);
    let mut list = load_members(&dir, id);
    let was_active = list.iter().any(|m| m.member_id_hex == hex && !m.revoked);
    // Blobs stored before this grant are keyed to a pre-grant epoch: re-store
    // them under the post-grant epoch and republish. The grant already
    // happened, so a re-key failure must not abort the invite — the interval
    // hoster leg re-runs population on its next pass.
    let mut sp = linxiv_share::load(&e2ee_dir(&dir), id).map_err(fetch_error)?;
    if sp.papers.iter().any(|p| p.pdf_blob.is_some()) {
        match share_sync::populate_pdf_blobs(state, &node, &dir, &mut sp, true).await {
            Ok(()) => e2ee_timeout(node.publish_secure(&sp), "secure publish").await?,
            Err(e) => eprintln!("share {id}: blob re-key after invite: {e}"),
        }
    }
    if let Some(m) = list.iter_mut().find(|m| m.member_id_hex == hex) {
        m.role = role_s.into();
        m.name = name;
        m.revoked = false;
        m.invite = Some(invite.clone());
    } else {
        list.push(MemberEntry {
            member_id_hex: hex,
            name,
            role: role_s.into(),
            invited_at: chrono::Utc::now().to_rfc3339(),
            revoked: false,
            invite: Some(invite.clone()),
        });
    }
    if let Err(e) = save_members(&dir, id, &list) {
        if !was_active {
            // Undo the fresh grant the sidecar failed to record.
            let _ = e2ee_timeout(node.revoke(id, member), "revoke").await;
        }
        return Err(ApiError::new(
            500,
            format!("could not persist members sidecar: {e}"),
        ));
    }
    Ok(json!({ "invite": invite }))
}

/// `GET /api/share/{id}/members` — the sidecar list, with a live `query_role`
/// truth-check per invited entry (no role after having been invited = revoked).
///
/// Co-admin (spec §1.1): keyhive supports granting a member Admin, but
/// management ops that force PCS rotation (revoke, downgrade) must run where
/// the doc is hosted — so membership management stays Hoster-only in the app
/// and co-admin is a supported-but-deferred capability, not built UI. Roles
/// offered here and on the role route are viewer/editor only.
async fn members(share: &ShareState, id: &str) -> Result<Value, ApiError> {
    let dir = share.share_dir().to_path_buf();
    ensure_e2ee_hosted(&dir, id)?;
    let node = share.node().await;
    let members = load_members(&dir, id);
    // One concurrent truth-check per live invited entry; each query keeps its
    // own timeout budget instead of serially stacking (entries × budget).
    // `Some(revoked_now)` = query answered; `None` = unqueryable, keep sidecar.
    let checks = futures_util::future::join_all(members.iter().map(|m| async {
        if m.revoked || m.role == "hoster" {
            return None;
        }
        let (Some(node), Some(mid)) = (&node, member_id_from_hex(&m.member_id_hex)) else {
            return None;
        };
        match e2ee_timeout(node.query_role(id, mid), "member query").await {
            Ok(role) => Some(role.is_none()),
            Err(_) => None,
        }
    }))
    .await;
    let mut out = Vec::new();
    for (m, check) in members.into_iter().zip(checks) {
        let mut revoked = m.revoked;
        let mut verified = m.role == "hoster";
        if let Some(revoked_now) = check {
            revoked = revoked_now;
            verified = true;
        }
        out.push(json!({
            "member_id": m.member_id_hex,
            "name": m.name,
            "role": m.role,
            "invited_at": m.invited_at,
            "revoked": revoked,
            "verified": verified,
            // Re-sendable while the grant stands; dropped once revoked.
            "invite": if revoked { Value::Null } else { json!(m.invite) },
        }));
    }
    Ok(json!({ "members": out }))
}

/// `POST /api/share/{id}/member/{mid}/role {role: "editor"|"viewer"}` — change
/// an invited member's role on a hoster-owned e2ee share. The capability layer
/// revokes + regrants (a downgrade rotates the project key), so stored PDF
/// blobs are re-keyed + republished afterwards, then the sidecar entry updates.
/// Admin/relay targets are refused: co-admin is app-deferred (see `members`).
async fn set_member_role(
    state: &AppState,
    share: &ShareState,
    id: &str,
    mid: &str,
    body: Option<&Value>,
) -> Result<Value, ApiError> {
    let dir = share.share_dir().to_path_buf();
    ensure_e2ee_hosted(&dir, id)?;
    let role_s = body
        .and_then(|b| b.get("role"))
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::new(422, "missing `role` in body"))?;
    let role = match role_s {
        "editor" => Role::Edit,
        "viewer" => Role::Read,
        // keyhive-supported, app-deferred (co-admin / relay, spec §1.1).
        "admin" | "hoster" | "relay" => {
            return Err(ApiError::new(400, "role must be \"editor\" or \"viewer\""))
        }
        _ => return Err(ApiError::new(422, "role must be \"editor\" or \"viewer\"")),
    };
    let member =
        member_id_from_hex(mid).ok_or_else(|| ApiError::new(422, "malformed member id"))?;
    let canon_hex = member_id_hex(&member);
    let active = load_members(&dir, id)
        .into_iter()
        .find(|m| m.member_id_hex == canon_hex && !m.revoked)
        .ok_or_else(|| ApiError::new(404, "member not found on this share"))?;
    if active.role == "hoster" {
        return Err(ApiError::new(409, "cannot change the host's role"));
    }
    let node = live_node(share).await?;
    if node.self_member_id().map(|s| s == member).unwrap_or(false) {
        return Err(ApiError::new(409, "cannot change your own role as host"));
    }
    let _lock = share.lock_writes(id).await;
    // A concurrent unpublish may have parked the doc between the entry check and the lock.
    ensure_e2ee_hosted(&dir, id)?;
    // Re-check under the lock: a concurrent revoke may have landed after the
    // entry check above — set_role on a member with no live delegation is a
    // fresh grant in the capability layer, silently re-admitting them.
    if !load_members(&dir, id)
        .iter()
        .any(|m| m.member_id_hex == canon_hex && !m.revoked)
    {
        return Err(ApiError::new(404, "member not found on this share"));
    }
    // ShareError::LastReader surfaces as 409 via fetch_error.
    e2ee_timeout(node.set_role(id, member, role), "role change").await?;
    // A downgrade rotated the project key: blobs stored under the old epoch
    // must re-key + republish. The role change already happened, so a re-key
    // failure must not abort the request — the interval hoster leg re-runs
    // population on its next pass (same contract as invite).
    let mut sp = linxiv_share::load(&e2ee_dir(&dir), id).map_err(fetch_error)?;
    if sp.papers.iter().any(|p| p.pdf_blob.is_some()) {
        match share_sync::populate_pdf_blobs(state, &node, &dir, &mut sp, true).await {
            Ok(()) => e2ee_timeout(node.publish_secure(&sp), "secure publish").await?,
            Err(e) => eprintln!("share {id}: blob re-key after role change: {e}"),
        }
    }
    let mut list = load_members(&dir, id);
    for m in list.iter_mut().filter(|m| m.member_id_hex == canon_hex) {
        m.role = role_s.into();
        // The regrant behind set_role invalidates the stored invite string.
        m.invite = None;
    }
    if let Err(e) = save_members(&dir, id, &list) {
        eprintln!("share {id}: could not persist members sidecar: {e}");
    }
    Ok(json!({ "member_id": canon_hex, "role": role_s }))
}

/// `POST /api/share/{id}/revoke {member_id}` — revoke a member (the project key
/// rotates) and mark the sidecar entry.
async fn revoke_member(
    share: &ShareState,
    id: &str,
    body: Option<&Value>,
) -> Result<Value, ApiError> {
    let dir = share.share_dir().to_path_buf();
    ensure_e2ee_hosted(&dir, id)?;
    let hex = body
        .and_then(|b| b.get("member_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::new(422, "missing `member_id` in body"))?;
    let mid = member_id_from_hex(hex).ok_or_else(|| ApiError::new(422, "malformed `member_id`"))?;
    let node = live_node(share).await?;
    if node.self_member_id().map(|s| s == mid).unwrap_or(false) {
        return Err(ApiError::new(409, "cannot revoke yourself as host"));
    }
    let canon_hex = member_id_hex(&mid);
    if load_members(&dir, id)
        .iter()
        .any(|m| m.member_id_hex == canon_hex && m.role == "hoster")
    {
        return Err(ApiError::new(409, "cannot revoke the host"));
    }
    let _lock = share.lock_writes(id).await;
    e2ee_timeout(node.revoke(id, mid), "revoke").await?;
    let mut list = load_members(&dir, id);
    for m in list.iter_mut().filter(|m| m.member_id_hex == canon_hex) {
        m.revoked = true;
        m.invite = None;
    }
    if let Err(e) = save_members(&dir, id, &list) {
        eprintln!("share {id}: could not persist members sidecar: {e}");
    }
    Ok(json!({ "revoked": true }))
}

/// `POST /api/share/{id}/rekey` — re-encrypt a hosted e2ee share's history
/// under the current epoch, then republish it (and re-key its PDF blobs) so
/// every current member can read everything.
///
/// Invites re-seal on their own; this repairs shares whose members were invited
/// after the content was already sealed, which leaves them fetching commits
/// they hold no key for (keyhive #136) with no way out but this.
async fn rekey(state: &AppState, share: &ShareState, id: &str) -> Result<Value, ApiError> {
    let dir = share.share_dir().to_path_buf();
    ensure_e2ee_hosted(&dir, id)?;
    let node = live_node(share).await?;
    let _lock = share.lock_writes(id).await;
    ensure_e2ee_hosted(&dir, id)?;
    e2ee_timeout(node.rekey_e2ee(id), "re-key").await?;
    // Blobs are sealed per epoch too, so a doc-only re-key would leave every
    // shared PDF unreadable to the same members.
    let mut sp = linxiv_share::load(&e2ee_dir(&dir), id).map_err(fetch_error)?;
    if sp.papers.iter().any(|p| p.pdf_blob.is_some()) {
        share_sync::populate_pdf_blobs(state, &node, &dir, &mut sp, true)
            .await
            .map_err(fetch_error)?;
    }
    e2ee_timeout(node.publish_secure(&sp), "secure publish").await?;
    println!(
        "share {id}: re-keyed and republished papers={} members={}",
        sp.papers.len(),
        live_member_count(&dir, id),
    );
    Ok(json!({ "rekeyed": true, "members": live_member_count(&dir, id) }))
}

/// `POST /api/share/{id}/member/{mid}/remove` — revoke, then drop the sidecar
/// row entirely, so a re-invite of the same device starts from a clean slate
/// (a revoked row keeps its stale role and dead invite string around).
/// Revoking first is what actually withdraws the capability; the row is
/// bookkeeping. Already-revoked members skip straight to the row delete.
async fn remove_member(share: &ShareState, id: &str, mid: &str) -> Result<Value, ApiError> {
    let dir = share.share_dir().to_path_buf();
    ensure_e2ee_hosted(&dir, id)?;
    let member =
        member_id_from_hex(mid).ok_or_else(|| ApiError::new(422, "malformed member id"))?;
    let canon_hex = member_id_hex(&member);
    let entry = load_members(&dir, id)
        .into_iter()
        .find(|m| m.member_id_hex == canon_hex)
        .ok_or_else(|| ApiError::new(404, "member not found on this share"))?;
    if entry.role == "hoster" {
        return Err(ApiError::new(409, "cannot remove the host"));
    }
    let node = live_node(share).await?;
    if node.self_member_id().map(|s| s == member).unwrap_or(false) {
        return Err(ApiError::new(409, "cannot remove yourself as host"));
    }
    let _lock = share.lock_writes(id).await;
    ensure_e2ee_hosted(&dir, id)?;
    if !entry.revoked {
        e2ee_timeout(node.revoke(id, member), "revoke").await?;
    }
    let mut list = load_members(&dir, id);
    list.retain(|m| m.member_id_hex != canon_hex);
    save_members(&dir, id, &list)
        .map_err(|e| ApiError::new(500, format!("could not persist members sidecar: {e}")))?;
    Ok(json!({ "removed": true, "member_id": canon_hex }))
}

/// `POST /api/share/{id}/pdf {source_id}` — fetch + decrypt a received e2ee
/// share's PDF blob and save it to the managed PDF dir, under the same
/// `pdf_save_limit_mb` total-storage cap the downloader/import paths enforce.
async fn shared_pdf(
    state: &AppState,
    share: &ShareState,
    id: &str,
    body: Option<&Value>,
) -> Result<Value, ApiError> {
    let source_id = body
        .and_then(|b| b.get("source_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::new(422, "missing `source_id` in body"))?
        .to_string();
    let sp = ShareNode::e2ee_received(share.share_dir(), id)?;
    let paper = sp
        .papers
        .iter()
        .find(|p| p.source_id == source_id)
        .ok_or_else(|| ApiError::new(404, format!("paper {source_id:?} not in share")))?;
    let ticket = paper
        .pdf_blob
        .clone()
        .ok_or_else(|| ApiError::new(404, "no PDF shared for this paper"))?;
    let version = paper.version;
    let pdf_dir = state.pdf_dir.clone();
    let dest = pdf_dir.join(pdf_on_disk_name(&source_id, version));
    // The paper row only exists once the share has been imported; the
    // crash-recovery re-register below needs it too.
    let row_exists = state
        .with_conn(|c| {
            paper_svc::get(
                c,
                &paper_svc::PaperRef::Source {
                    source_id: source_id.clone(),
                    version: Some(version),
                },
            )
        })?
        .is_some();
    if !row_exists {
        return Err(ApiError::new(
            409,
            "import the share before downloading PDFs",
        ));
    }
    if dest.is_file() {
        let path = dest.to_string_lossy().into_owned();
        // Re-registers a file left by a crash between rename and mark.
        state.with_conn(|c| paper_svc::mark_pdf_saved(c, &source_id, &path, version))?;
        return Ok(json!({ "source_id": source_id, "version": version, "path": path }));
    }
    let node = live_node(share).await?;
    // Remaining pdf quota caps the transport fetch (413 past it).
    let max = config::UserSettings::load()?.pdf_save_limit_bytes();
    let remaining = max.saturating_sub(linxiv_core::service::files::pdf_storage_bytes(&pdf_dir));
    let bytes = tokio::time::timeout(
        SHARE_NET_TIMEOUT * 2,
        node.read_pdf_blob(id, &ticket, remaining),
    )
    .await
    .map_err(|_| ApiError::new(504, "shared PDF fetch timed out"))??;
    let _lock = share.lock_writes(id).await;
    let mut wrote = false;
    if !dest.is_file() {
        paper_import::check_pdf_storage_quota(&pdf_dir, bytes.len(), max)?;
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(&pdf_dir)?;
            let tmp = dest.with_extension(format!("pdf.{id}.tmp"));
            std::fs::write(&tmp, &bytes)?;
            std::fs::rename(&tmp, &dest)
        };
        write().map_err(|e| ApiError::new(500, format!("could not save shared PDF: {e}")))?;
        linxiv_core::service::files::note_pdf_written(&dest, bytes.len() as u64);
        wrote = true;
    }
    let path = dest.to_string_lossy().into_owned();
    if let Err(e) = state.with_conn(|c| paper_svc::mark_pdf_saved(c, &source_id, &path, version)) {
        // Only clean up a file this request wrote, not one a concurrent request saved.
        if wrote {
            linxiv_core::service::files::remove_pdf_counted(&dest);
        }
        return Err(ApiError::new(500, e.to_string()));
    }
    Ok(json!({ "source_id": source_id, "version": version, "path": path }))
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

    /// A placeholder mirror from an offline join holds an empty doc that never
    /// hydrates; it must still list, flagged pending, or the join is invisible.
    #[test]
    fn pending_mirror_is_listed() {
        let (state, _pid) = seeded_state();
        let dir = tempfile::tempdir().unwrap();
        let share = ShareState::new(dir.path());
        let id = "11111111-2222-4333-8444-555555555555";
        let rec = e2ee_received_dir(dir.path());
        std::fs::create_dir_all(&rec).unwrap();
        std::fs::write(doc_path(&rec, id), automerge::AutoCommit::new().save()).unwrap();

        let listed = list_received(&state, &share).unwrap();
        let entries = listed["received"].as_array().unwrap();
        assert_eq!(entries.len(), 1, "pending mirror must surface");
        assert_eq!(entries[0]["share_id"], json!(id));
        assert_eq!(entries[0]["pending"], json!(true));
        assert_eq!(entries[0]["synced_at"], Value::Null);
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
                pdf_blob: None,
                author_orcids: vec![],
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

            let paper = paper_svc::get(c, &paper_svc::PaperRef::source("arxiv:9".into()))
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
    async fn unlink_clears_link_keeps_project_and_mirror() {
        let state = empty_state();
        let dir = tempfile::tempdir().unwrap();
        let share = ShareState::new(dir.path());
        save(&received_dir(dir.path()), &remote_shared(SID, "b")).unwrap();
        let fk = share_sync::import_received(&state, dir.path(), SID).unwrap();

        let v = unlink(&state, &share, SID).await.unwrap();

        assert_eq!(v["unlinked"], json!(true));
        state.with_conn(|c| {
            assert_eq!(project_svc::find_by_share_id(c, SID).unwrap(), None);
            // The project itself survives the unlink.
            assert!(project_svc::get(
                c,
                &project_svc::Project {
                    project_fk: Some(fk)
                },
            )
            .unwrap()
            .is_some());
        });
        assert!(doc_path(&received_dir(dir.path()), SID).exists());
        // Idempotent: a second unlink reports no link; unknown id → 404.
        let v = unlink(&state, &share, SID).await.unwrap();
        assert_eq!(v["unlinked"], json!(false));
        assert_eq!(
            unlink(&state, &share, "44444444-4444-4444-8444-444444444444")
                .await
                .unwrap_err()
                .status,
            404
        );
    }

    #[tokio::test]
    async fn unlink_refuses_hoster_share() {
        // A hosted doc lives at the share-dir root, not under received/ —
        // unlink must 404 and leave the publish-identity link intact.
        let mut conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        let fk = project_svc::create(
            &mut conn,
            &linxiv_core::models::ProjectIn {
                name: "Hosted".into(),
                description: String::new(),
                color: None,
                tags: vec![],
                source_fks: vec![],
            },
        )
        .unwrap();
        project_svc::adopt_share_id(&conn, fk, SID).unwrap();
        let state = AppState::from_parts(conn, std::env::temp_dir(), std::env::temp_dir());
        let dir = tempfile::tempdir().unwrap();
        let share = ShareState::new(dir.path());
        save(dir.path(), &remote_shared(SID, "b")).unwrap();

        assert_eq!(unlink(&state, &share, SID).await.unwrap_err().status, 404);
        state.with_conn(|c| {
            assert_eq!(project_svc::find_by_share_id(c, SID).unwrap(), Some(fk));
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

    // ── W4: e2ee arms ────────────────────────────────────────────────────────

    // Keyhive/BeeKEM ops are slow in debug builds; generous per-op budget.
    async fn slow<T>(fut: impl std::future::Future<Output = T>) -> T {
        tokio::time::timeout(Duration::from_secs(60), fut)
            .await
            .expect("e2ee op should not hang on loopback")
    }

    #[test]
    fn members_sidecar_corruption_is_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_members(dir.path(), "s-1").is_empty());
        std::fs::create_dir_all(dir.path().join("members")).unwrap();
        std::fs::write(members_path(dir.path(), "s-1"), b"{ not json").unwrap();
        assert!(load_members(dir.path(), "s-1").is_empty());
    }

    #[tokio::test]
    async fn join_garbage_is_400_with_both_parse_errors() {
        let dir = tempfile::tempdir().unwrap();
        let share = ShareState::new(dir.path());
        let err = join(&share, Some(&json!({ "ticket": "garbage" })))
            .await
            .unwrap_err();
        assert_eq!(err.status, 400);
        assert!(
            err.detail.contains("not a share ticket or invite"),
            "{}",
            err.detail
        );
    }

    // Role-route input validation, cheap (no node, store-only state).
    #[tokio::test]
    async fn set_member_role_validates_before_network() {
        let state = empty_state();
        let dir = tempfile::tempdir().unwrap();
        let share = ShareState::new(dir.path());
        let viewer = json!({ "role": "viewer" });

        // Not a hosted e2ee share → 404.
        let err = set_member_role(&state, &share, SID, "ab", Some(&viewer))
            .await
            .unwrap_err();
        assert_eq!(err.status, 404);

        save(&e2ee_dir(dir.path()), &remote_shared(SID, "b")).unwrap();
        // Admin/relay targets refused: co-admin is app-deferred (spec §1.1).
        let err = set_member_role(&state, &share, SID, "ab", Some(&json!({ "role": "admin" })))
            .await
            .unwrap_err();
        assert_eq!(err.status, 400);
        // Unknown role word → 422.
        let err = set_member_role(&state, &share, SID, "ab", Some(&json!({ "role": "owner" })))
            .await
            .unwrap_err();
        assert_eq!(err.status, 422);
        // Malformed member id → 422.
        let err = set_member_role(&state, &share, SID, "zz", Some(&viewer))
            .await
            .unwrap_err();
        assert_eq!(err.status, 422);
        // Well-formed id that was never invited → 404 (never a blind grant).
        let hex = "aa".repeat(32);
        let err = set_member_role(&state, &share, SID, &hex, Some(&viewer))
            .await
            .unwrap_err();
        assert_eq!(err.status, 404);
        // Invited member, but store-only state has no live node → 503.
        save_members(
            dir.path(),
            SID,
            &[MemberEntry {
                member_id_hex: hex.clone(),
                name: None,
                role: "viewer".into(),
                invited_at: chrono::Utc::now().to_rfc3339(),
                revoked: false,
                invite: None,
            }],
        )
        .unwrap();
        let err = set_member_role(
            &state,
            &share,
            SID,
            &hex,
            Some(&json!({ "role": "editor" })),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 503);
    }

    #[tokio::test]
    async fn shared_pdf_without_blob_is_404() {
        let state = empty_state();
        let dir = tempfile::tempdir().unwrap();
        let share = ShareState::new(dir.path());
        save(&e2ee_received_dir(dir.path()), &remote_shared(SID, "b")).unwrap();

        let body = json!({ "source_id": "arxiv:9" });
        let err = shared_pdf(&state, &share, SID, Some(&body))
            .await
            .unwrap_err();
        assert_eq!(err.status, 404);
        assert!(err.detail.contains("no PDF"), "{}", err.detail);

        let body = json!({ "source_id": "nope" });
        let err = shared_pdf(&state, &share, SID, Some(&body))
            .await
            .unwrap_err();
        assert_eq!(err.status, 404);
    }

    // Full e2ee arm flow over loopback: publish_secure (storing the PDF blob),
    // invite, join-by-invite through the shared join arm, members truth-check,
    // shared_pdf save on the reader, revoke.
    #[tokio::test(flavor = "multi_thread")]
    async fn e2ee_arms_roundtrip_over_loopback() {
        let a_dir = tempfile::tempdir().unwrap();
        let b_dir = tempfile::tempdir().unwrap();
        let a_pdf = tempfile::tempdir().unwrap();
        let b_pdf = tempfile::tempdir().unwrap();

        // A's canonical project: one paper whose managed PDF is on disk.
        let mut conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        let pid = linxiv_share::import_shared_project(&mut conn, &remote_shared(SID, "b")).unwrap();
        let state_a = AppState::from_parts(conn, a_pdf.path().to_path_buf(), std::env::temp_dir());
        let pdf_bytes = b"%PDF-1.7 shared".to_vec();
        std::fs::write(
            a_pdf.path().join(pdf_on_disk_name("arxiv:9", 1)),
            &pdf_bytes,
        )
        .unwrap();
        let mut conn_b = storage::open_in_memory().unwrap();
        storage::init_db(&conn_b).unwrap();
        // B's row for the shared paper, as `join` + import would leave it, so the
        // download gate in `shared_pdf` finds it.
        linxiv_share::import_shared_project(&mut conn_b, &remote_shared(SID, "b")).unwrap();
        let state_b =
            AppState::from_parts(conn_b, b_pdf.path().to_path_buf(), std::env::temp_dir());

        let node_a = ShareNode::bind_offline(a_dir.path(), &a_dir.path().join("p2p"))
            .await
            .unwrap();
        let node_b = ShareNode::bind_offline(b_dir.path(), &b_dir.path().join("p2p"))
            .await
            .unwrap();
        let share_a = ShareState::with_node(a_dir.path(), node_a);
        let share_b = ShareState::with_node(b_dir.path(), node_b);

        // publish_secure: e2ee doc with the blob ticket + hoster sidecar entry.
        let resp = slow(publish_secure(&state_a, &share_a, &pid.to_string()))
            .await
            .unwrap();
        assert_eq!(resp["share_id"], json!(SID));
        assert_eq!(resp["e2ee"], json!(true));
        let doc = linxiv_share::load(&linxiv_share::e2ee_dir(a_dir.path()), SID).unwrap();
        assert!(doc.papers[0].pdf_blob.is_some(), "pdf blob ticket stored");
        let sidecar = load_members(a_dir.path(), SID);
        assert_eq!(sidecar.len(), 1);
        assert_eq!(sidecar[0].role, "hoster");

        // Summary carries the e2ee flag + member_count (invited members only,
        // hoster excluded).
        let listed = list_shared(&state_a, &share_a).unwrap();
        let entry = &listed["shared_projects"][0];
        assert_eq!(entry["e2ee"], json!(true));
        assert_eq!(entry["member_count"], json!(0));

        // Invite B as viewer using B's member code.
        let code = member_code(&share_b).await.unwrap()["code"]
            .as_str()
            .unwrap()
            .to_string();
        let body = json!({ "member_code": code, "role": "viewer", "name": "Bee" });
        let inv = slow(invite(&state_a, &share_a, SID, Some(&body)))
            .await
            .unwrap()["invite"]
            .as_str()
            .unwrap()
            .to_string();

        // B pastes the invite into the SAME join arm the plain flow uses.
        let joined = slow(join(&share_b, Some(&json!({ "ticket": inv }))))
            .await
            .unwrap();
        assert_eq!(joined["share_id"], json!(SID));
        assert_eq!(joined["e2ee"], json!(true));

        // B's received listing annotates the live-checked role (spec §7).
        let listed = slow(list_received_with_role(&state_b, &share_b))
            .await
            .unwrap();
        let entry = &listed["received"][0];
        assert_eq!(entry["e2ee"], json!(true));
        assert_eq!(entry["role"], json!("viewer"));

        // members: hoster + live-checked viewer.
        let m = slow(members(&share_a, SID)).await.unwrap();
        let list = m["members"].as_array().unwrap().clone();
        assert_eq!(list.len(), 2);
        let viewer = list.iter().find(|m| m["role"] == json!("viewer")).unwrap();
        assert_eq!(viewer["name"], json!("Bee"));
        assert_eq!(viewer["revoked"], json!(false));
        let member_id = viewer["member_id"].as_str().unwrap().to_string();

        // §3.3 role route: promote the viewer to editor and back; the
        // sidecar reflects each change (query_role verified via members()).
        for target in ["editor", "viewer"] {
            let changed = slow(set_member_role(
                &state_a,
                &share_a,
                SID,
                &member_id,
                Some(&json!({ "role": target })),
            ))
            .await
            .unwrap();
            assert_eq!(changed["role"], json!(target));
            let m = slow(members(&share_a, SID)).await.unwrap();
            let bee = m["members"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["member_id"] == json!(member_id.clone()))
                .unwrap()
                .clone();
            assert_eq!(bee["role"], json!(target));
            assert_eq!(bee["revoked"], json!(false));
        }

        // B saves the shared PDF through the cap-checked path.
        let body = json!({ "source_id": "arxiv:9" });
        let saved = slow(shared_pdf(&state_b, &share_b, SID, Some(&body)))
            .await
            .unwrap();
        let path = saved["path"].as_str().unwrap();
        assert_eq!(std::fs::read(path).unwrap(), pdf_bytes);
        assert!(path.starts_with(b_pdf.path().to_str().unwrap()));

        // Revoke the viewer; the members list reports it.
        let body = json!({ "member_id": member_id });
        slow(revoke_member(&share_a, SID, Some(&body)))
            .await
            .unwrap();
        let m = slow(members(&share_a, SID)).await.unwrap();
        let viewer = m["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["role"] == json!("viewer"))
            .unwrap()
            .clone();
        assert_eq!(viewer["revoked"], json!(true));

        share_a.shutdown().await.unwrap();
        share_b.shutdown().await.unwrap();
    }
}
