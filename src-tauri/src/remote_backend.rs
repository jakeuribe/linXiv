//! Remote Query Mode, client half (CONTEXT.md: Library Backend / Remote
//! Query Mode / Node Address): the persistent registry of remote backends,
//! the `api_remote` command that speaks `linxiv-api/1` to one of them, and
//! the byte-lane PDF fetch with a local cache.
//!
//! Every request names its backend explicitly — the backend is a parameter
//! of the request context, never a global read inside transport code; the
//! PoC "default backend" lives in the UI layer only. Outbound dials reuse
//! the app's existing share endpoint ([`ShareState`]) — one endpoint, never
//! a second bind.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use linxiv_core::config::{self, UserSettings};
use linxiv_core::service::paper::pdf_on_disk_name;
use linxiv_p2p::{api, ApiClientError, ByteLane, Connection, Endpoint, EndpointAddr, NodeAddress};

use crate::route::share::ShareState;
use crate::route::ApiRequest;

/// UserSettings key holding the backend registry (reuses the existing
/// settings persistence; no new storage system).
const SETTINGS_KEY: &str = "remote_backends";

/// The ONE honest state for refused-or-offline (CONTEXT.md: a non-admitted
/// device is refused indistinguishably from an offline node, by design).
const UNREACHABLE: &str =
    "can't reach this node — it may be offline, or this device isn't admitted yet";

/// One registered remote Library Backend. `node_address` is the pasted
/// `linxivnode…` locator string (a locator, not a capability — the node's
/// Member List decides access). `id` is app-generated (`b<N>`), never
/// derived from remote input, and doubles as the PDF-cache path segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Backend {
    pub id: String,
    pub label: String,
    pub node_address: String,
}

/// Typed failure the webview can distinguish: `{kind: "unreachable" |
/// "remote" | "transport" | "invalid", ...}`.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RemoteError {
    /// Dial failed or the node closed without answering — offline and
    /// not-admitted are one state on purpose.
    Unreachable { detail: String },
    /// The node answered with an error envelope.
    Remote { status: u16, detail: String },
    /// Transport-level failure that is not a refusal (or a local I/O fault).
    Transport { detail: String },
    /// Bad input: unknown backend id, unparseable address.
    Invalid { detail: String },
}

impl RemoteError {
    fn invalid(detail: impl Into<String>) -> Self {
        Self::Invalid {
            detail: detail.into(),
        }
    }
    fn transport(detail: impl Into<String>) -> Self {
        Self::Transport {
            detail: detail.into(),
        }
    }
    fn unreachable() -> Self {
        Self::Unreachable {
            detail: UNREACHABLE.into(),
        }
    }
}

fn client_err(e: ApiClientError) -> RemoteError {
    match e {
        ApiClientError::Refused => RemoteError::unreachable(),
        ApiClientError::Other(e) => RemoteError::transport(e.to_string()),
    }
}

// ── backend registry (persisted in user settings) ───────────────────────────

fn parse_backends(v: Option<&Value>) -> Vec<Backend> {
    v.and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

fn load_backends() -> Result<Vec<Backend>, RemoteError> {
    let settings = UserSettings::load().map_err(|e| RemoteError::transport(e.to_string()))?;
    Ok(parse_backends(settings.get(SETTINGS_KEY)))
}

fn save_backends(list: &[Backend]) -> Result<(), RemoteError> {
    UserSettings::load()
        .and_then(|mut s| s.set(SETTINGS_KEY, serde_json::to_value(list).unwrap_or_default()))
        .map_err(|e| RemoteError::transport(e.to_string()))
}

/// Smallest unused `b<N>` slug — app-generated, filesystem-safe.
fn new_id(existing: &[Backend]) -> String {
    (1..)
        .map(|n| format!("b{n}"))
        .find(|id| !existing.iter().any(|b| &b.id == id))
        .expect("unbounded id space")
}

/// `GET`-shaped registry listing.
#[tauri::command]
pub fn remote_backends_list() -> Result<Vec<Backend>, RemoteError> {
    load_backends()
}

/// Store a backend after validating the pasted address parses. Adding does
/// not dial — reachability is a per-request question.
#[tauri::command]
pub fn remote_backend_add(label: String, address: String) -> Result<Backend, RemoteError> {
    let address = address.trim().to_string();
    address
        .parse::<NodeAddress>()
        .map_err(|e| RemoteError::invalid(format!("not a node address: {e}")))?;
    let mut list = load_backends()?;
    let backend = Backend {
        id: new_id(&list),
        label,
        node_address: address,
    };
    list.push(backend.clone());
    save_backends(&list)?;
    Ok(backend)
}

/// Remove a backend, its cached connection, and its PDF cache directory.
/// The cache MUST go: `new_id` refills freed `b<N>` slugs, so a leftover
/// `remote_pdf_cache/{id}` would be served as a later backend's PDFs
/// (cross-backend cache poisoning). Purge failure aborts the removal.
#[tauri::command]
pub async fn remote_backend_remove(
    remote: tauri::State<'_, RemoteState>,
    id: String,
) -> Result<(), RemoteError> {
    let mut list = load_backends()?;
    let before = list.len();
    list.retain(|b| b.id != id);
    if list.len() == before {
        return Err(RemoteError::invalid(format!("unknown backend {id:?}")));
    }
    purge_pdf_cache(&config::data_dir().join("remote_pdf_cache"), &id)
        .map_err(|e| RemoteError::transport(format!("clearing pdf cache: {e}")))?;
    save_backends(&list)?;
    remote.conns.lock().await.remove(&id);
    Ok(())
}

/// Delete `root/{id}`; an absent dir is success. Non-slug ids (possible only
/// via a hand-edited registry) are a no-op — [`fetch_remote_pdf`] refuses
/// them, so nothing was ever cached under one, and they must not become a
/// path segment here either.
fn purge_pdf_cache(root: &Path, id: &str) -> std::io::Result<()> {
    if !is_slug(id) {
        return Ok(());
    }
    match std::fs::remove_dir_all(root.join(id)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// App-generated ids are `b<N>`, but the registry file is hand-editable —
/// anything used as a path segment must pass this.
fn is_slug(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

// ── connection cache + request core ─────────────────────────────────────────

/// Managed beside `AppState`/`ShareState`: one cached connection per backend
/// id. Streams are per-request, so concurrent requests share a connection.
#[derive(Default)]
pub struct RemoteState {
    conns: tokio::sync::Mutex<HashMap<String, Connection>>,
}

/// Cached connection for `backend_id`, dialing `addr` when there is none
/// (or when `fresh` forces a redial). A dial failure is [`UNREACHABLE`].
// ponytail: two concurrent first requests may both dial; the second insert
// wins and both connections work — dedupe only if dials ever get expensive.
async fn conn_for(
    ep: &Endpoint,
    remote: &RemoteState,
    backend_id: &str,
    addr: EndpointAddr,
    fresh: bool,
) -> Result<Connection, RemoteError> {
    if !fresh {
        if let Some(c) = remote.conns.lock().await.get(backend_id) {
            if c.close_reason().is_none() {
                return Ok(c.clone());
            }
        }
    }
    // Bound the dial: iroh keeps trying paths indefinitely, so a dead or
    // unadmitted node would otherwise spin the UI forever.
    let c = tokio::time::timeout(std::time::Duration::from_secs(15), api::connect(ep, addr))
        .await
        .map_err(|_| RemoteError::unreachable())?
        .map_err(|_| RemoteError::unreachable())?;
    remote
        .conns
        .lock()
        .await
        .insert(backend_id.to_string(), c.clone());
    Ok(c)
}

/// One JSON round trip against a backend, reconnecting once on failure.
// ponytail: the blind retry can duplicate a non-idempotent write whose
// response was lost; make writes idempotent server-side if that ever bites.
pub async fn request_remote(
    ep: &Endpoint,
    remote: &RemoteState,
    backend_id: &str,
    addr: EndpointAddr,
    req: &Value,
) -> Result<Value, RemoteError> {
    let conn = conn_for(ep, remote, backend_id, addr.clone(), false).await?;
    let env = match api::request(&conn, req).await {
        Ok(env) => env,
        Err(_) => {
            let conn = conn_for(ep, remote, backend_id, addr, true).await?;
            api::request(&conn, req).await.map_err(client_err)?
        }
    };
    unwrap_envelope(env)
}

/// Byte-lane round trip with the same reconnect-once policy (GET-only, safe
/// to retry).
async fn request_bytes_remote(
    ep: &Endpoint,
    remote: &RemoteState,
    backend_id: &str,
    addr: EndpointAddr,
    req: &Value,
) -> Result<(Value, ByteLane), RemoteError> {
    let conn = conn_for(ep, remote, backend_id, addr.clone(), false).await?;
    match api::request_bytes(&conn, req).await {
        Ok(r) => Ok(r),
        Err(_) => {
            let conn = conn_for(ep, remote, backend_id, addr, true).await?;
            api::request_bytes(&conn, req).await.map_err(client_err)
        }
    }
}

/// Split the node's envelope: 2xx yields `body`, anything else the typed
/// remote error.
fn unwrap_envelope(mut env: Value) -> Result<Value, RemoteError> {
    let status = env.get("status").and_then(Value::as_u64).unwrap_or(0) as u16;
    if (200..300).contains(&status) {
        Ok(env.get_mut("body").map(Value::take).unwrap_or(Value::Null))
    } else {
        Err(RemoteError::Remote {
            status,
            detail: env
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or("remote error")
                .to_string(),
        })
    }
}

/// Registry lookup + live-endpoint resolution for one request. Returns the
/// node (borrow its endpoint) and the dialable address.
async fn resolve(
    share: &ShareState,
    backend_id: &str,
) -> Result<(Arc<linxiv_share::ShareNode>, EndpointAddr), RemoteError> {
    let backend = load_backends()?
        .into_iter()
        .find(|b| b.id == backend_id)
        .ok_or_else(|| RemoteError::invalid(format!("unknown backend {backend_id:?}")))?;
    let addr: NodeAddress = backend
        .node_address
        .parse()
        .map_err(|e| RemoteError::invalid(format!("stored node address is invalid: {e}")))?;
    let node = share
        .node()
        .await
        .ok_or_else(|| RemoteError::transport("p2p transport is not initialized"))?;
    Ok((node, addr.endpoint_addr()))
}

/// The remote twin of the `api` command: same `ApiRequest`, addressed to a
/// registered backend, returning the envelope's `body`. Remote mode is
/// online-only; `pdf-path` is denied by the node (403) — remote PDFs go
/// through [`remote_pdf`].
#[tauri::command]
pub async fn api_remote(
    share: tauri::State<'_, ShareState>,
    remote: tauri::State<'_, RemoteState>,
    backend_id: String,
    req: ApiRequest,
) -> Result<Value, RemoteError> {
    let (node, addr) = resolve(&share, &backend_id).await?;
    let req = serde_json::to_value(&req).map_err(|e| RemoteError::transport(e.to_string()))?;
    request_remote(node.endpoint(), &remote, &backend_id, addr, &req).await
}

// ── remote PDFs (byte lane + local cache) ───────────────────────────────────

/// `%`-escape everything but unreserved chars — `encodeURIComponent` for the
/// source-id path segment (old-style ids carry `/`, prefixed ones `:`).
fn pct_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Byte-lane read deadline from the header's own `eta_seconds` (a paced
/// transfer legitimately takes that long), doubled plus slack so a slow link
/// never trips it — but a hostile/hung node can't pin the UI forever. A
/// missing or absurd eta gets one generous flat ceiling instead.
fn read_deadline(header: &Value) -> std::time::Duration {
    match header.get("eta_seconds").and_then(Value::as_f64) {
        Some(eta) if eta.is_finite() && (0.0..86_400.0).contains(&eta) => {
            std::time::Duration::from_secs_f64(eta * 2.0 + 30.0)
        }
        _ => std::time::Duration::from_secs(600),
    }
}

/// Byte-lane PDF fetch cached under `cache_root/{backend_id}/{name}`, where
/// `name` reuses core's `pdf_on_disk_name` sanitizer (path-traversal safety)
/// and `backend_id` is our own slug. Serves from cache when the file exists.
pub async fn fetch_remote_pdf(
    ep: &Endpoint,
    remote: &RemoteState,
    backend_id: &str,
    addr: EndpointAddr,
    source_id: &str,
    version: Option<u32>,
    cache_root: &Path,
) -> Result<PathBuf, RemoteError> {
    // Belt: refuse a non-slug id before it becomes a path segment.
    if !is_slug(backend_id) {
        return Err(RemoteError::invalid("backend id is not a slug"));
    }
    // ponytail: "latest" (no version) caches as v0 and never revalidates;
    // pass an explicit version to pin, or delete the cache file to refresh.
    let name = pdf_on_disk_name(source_id, version.map(i64::from).unwrap_or(0));
    let dir = cache_root.join(backend_id);
    let path = dir.join(name);
    if path.is_file() {
        return Ok(path);
    }
    let q = version.map(|v| format!("?version={v}")).unwrap_or_default();
    let req = json!({
        "method": "GET",
        "path": format!("/api/papers/{}/pdf{q}", pct_encode(source_id)),
        "body": Value::Null,
    });
    let (header, lane) = request_bytes_remote(ep, remote, backend_id, addr, &req).await?;
    let deadline = read_deadline(&header);
    // An answered error ships a bare envelope and an empty lane.
    unwrap_envelope(header)?;
    let bytes = tokio::time::timeout(deadline, lane.read_to_vec())
        .await
        .map_err(|_| RemoteError::transport("timed out reading pdf from the node"))?
        .map_err(|e| RemoteError::transport(e.to_string()))?;
    std::fs::create_dir_all(&dir).map_err(|e| RemoteError::transport(e.to_string()))?;
    // Sibling tmp + rename (same pattern as save_members); pid + counter keeps
    // concurrent fetches of the same paper from clobbering each other's tmp.
    static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("tmp-{}-{seq}", std::process::id()));
    std::fs::write(&tmp, &bytes)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| RemoteError::transport(format!("caching pdf: {e}")))?;
    Ok(path)
}

/// Fetch (or serve from cache) a remote paper's PDF and return the local
/// path — the existing path-based viewer machinery takes over from there.
#[tauri::command]
pub async fn remote_pdf(
    share: tauri::State<'_, ShareState>,
    remote: tauri::State<'_, RemoteState>,
    backend_id: String,
    source_id: String,
    version: Option<u32>,
) -> Result<String, RemoteError> {
    let (node, addr) = resolve(&share, &backend_id).await?;
    let root = config::data_dir().join("remote_pdf_cache");
    let path = fetch_remote_pdf(
        node.endpoint(),
        &remote,
        &backend_id,
        addr,
        &source_id,
        version,
        &root,
    )
    .await?;
    Ok(path.to_string_lossy().into_owned())
}

/// This device's member code — the iroh endpoint id an operator adds to
/// their node's Member List. NOTE: distinct from `GET /api/share/member_code`
/// (the keyhive e2ee member id); Remote Query admission keys on the
/// transport endpoint id.
#[tauri::command]
pub async fn remote_member_code(
    share: tauri::State<'_, ShareState>,
) -> Result<String, RemoteError> {
    share
        .endpoint_id()
        .await
        .ok_or_else(|| RemoteError::transport("p2p transport is not initialized"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Registry round trip is pure serde (the settings file itself is
    /// UserSettings' tested territory; tests never redirect the data dir).
    #[test]
    fn registry_round_trips_through_settings_value() {
        let list = vec![
            Backend {
                id: "b1".into(),
                label: "Lab node".into(),
                node_address: "linxivnodeabc".into(),
            },
            Backend {
                id: "b2".into(),
                label: "Home".into(),
                node_address: "linxivnodedef".into(),
            },
        ];
        let v = serde_json::to_value(&list).unwrap();
        assert_eq!(parse_backends(Some(&v)), list);
        // Missing/corrupt key degrades to an empty registry.
        assert_eq!(parse_backends(None), vec![]);
        assert_eq!(parse_backends(Some(&json!("junk"))), vec![]);
        // Slugs fill the smallest hole and stay filesystem-safe.
        assert_eq!(new_id(&list), "b3");
        assert_eq!(new_id(&list[1..]), "b1");
    }

    /// Removal must delete the cache dir: `new_id` reuses freed slugs, so a
    /// leftover dir would poison the next backend that gets the same id.
    #[test]
    fn purge_pdf_cache_deletes_dir_tolerates_absence_refuses_non_slugs() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("b1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("arxiv_2204.12985v1.pdf"), b"pdf").unwrap();
        purge_pdf_cache(root.path(), "b1").unwrap();
        assert!(!dir.exists());
        // Absent dir (nothing was ever cached) is success, not an error.
        purge_pdf_cache(root.path(), "b1").unwrap();
        // Non-slug id never becomes a path segment.
        std::fs::create_dir_all(root.path().join("evil")).unwrap();
        purge_pdf_cache(root.path(), "../evil").unwrap();
        assert!(root.path().join("evil").exists());
    }

    #[test]
    fn unwrap_envelope_splits_body_and_error() {
        assert_eq!(
            unwrap_envelope(json!({"status": 200, "body": {"ok": 1}})).unwrap(),
            json!({"ok": 1})
        );
        match unwrap_envelope(json!({"status": 403, "detail": "nope"})) {
            Err(RemoteError::Remote {
                status: 403,
                detail,
            }) => assert_eq!(detail, "nope"),
            other => panic!("got {other:?}"),
        }
        // A malformed envelope is an error, never a silent Ok(null).
        assert!(unwrap_envelope(json!({"detail": "x"})).is_err());
    }

    /// Deadline logic only — a live hang test would sleep >=30s (the floor
    /// baked into eta*2+30), so timeout firing is left to tokio's own tests.
    #[test]
    fn read_deadline_scales_with_eta_and_caps_garbage() {
        let d = |v: Value| read_deadline(&v).as_secs_f64();
        assert_eq!(d(json!({"eta_seconds": 10.0})), 50.0);
        assert_eq!(d(json!({"eta_seconds": 0.0})), 30.0);
        // Missing, non-numeric, negative, absurd, or non-finite eta falls
        // back to the flat ceiling instead of an unbounded (or zero) wait.
        for h in [
            json!({}),
            json!({"eta_seconds": "soon"}),
            json!({"eta_seconds": -5.0}),
            json!({"eta_seconds": 1e12}),
            json!({"eta_seconds": f64::NAN}),
        ] {
            assert_eq!(d(h), 600.0);
        }
    }

    #[test]
    fn pct_encode_matches_encodeuricomponent_for_source_ids() {
        assert_eq!(pct_encode("arxiv:2204.12985"), "arxiv%3A2204.12985");
        assert_eq!(pct_encode("math-ph/0309136"), "math-ph%2F0309136");
        assert_eq!(pct_encode("2204.12985"), "2204.12985");
        assert_eq!(crate::route::pct_decode(&pct_encode("a b/c:d")), "a b/c:d");
    }
}

/// In-process integration: this module's client core against the node half's
/// `linxiv-api/1` handler over the real transport — the same pairing the
/// desktop app and a headless node run.
#[cfg(test)]
mod proto_tests {
    use super::*;
    use crate::remote_query::{build_api_proto, Member, Role};
    use crate::state::AppState;
    use iroh::{endpoint::presets, protocol::Router};
    use linxiv_core::models::PaperMetadata;
    use linxiv_core::service::paper as svc_paper;

    fn state(pdf_dir: &Path) -> Arc<AppState> {
        let conn = linxiv_core::storage::open_in_memory().unwrap();
        linxiv_core::storage::init_db(&conn).unwrap();
        Arc::new(AppState::from_parts(
            conn,
            pdf_dir.to_path_buf(),
            std::env::temp_dir(),
        ))
    }

    /// Lean twin of `remote_query::proto_tests::serve` (that harness is
    /// module-private): the real handler behind an injected member list.
    async fn serve(state: Arc<AppState>, members: Vec<Member>) -> (Router, EndpointAddr) {
        let proto = build_api_proto(
            state,
            Arc::new(move |peer: &str| {
                members
                    .iter()
                    .find(|m| m.id.eq_ignore_ascii_case(peer) && m.role != Role::None)
                    .cloned()
            }),
            Arc::new(|_| {}),
            Arc::new(|_, _| {}),
            1_000_000,
        );
        let server = Endpoint::builder(presets::Minimal).bind().await.unwrap();
        let router = Router::builder(server).accept(api::ALPN, proto).spawn();
        let addr = router.endpoint().addr();
        (router, addr)
    }

    async fn client() -> Endpoint {
        Endpoint::builder(presets::Minimal).bind().await.unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn api_remote_round_trips_stats_and_reuses_the_connection() {
        let dir = tempfile::tempdir().unwrap();
        let ep = client().await;
        let (router, addr) = serve(
            state(dir.path()),
            vec![Member {
                id: ep.id().to_string(),
                role: Role::ReadWrite,
            }],
        )
        .await;
        let remote = RemoteState::default();
        let req = json!({ "method": "GET", "path": "/api/stats", "body": Value::Null });
        let body = request_remote(&ep, &remote, "b1", addr.clone(), &req)
            .await
            .unwrap();
        assert_eq!(body["paper_count"], 0);
        // Second request rides the cached connection.
        let body = request_remote(&ep, &remote, "b1", addr, &req)
            .await
            .unwrap();
        assert_eq!(body["paper_count"], 0);
        assert_eq!(remote.conns.lock().await.len(), 1);
        // A remote error envelope surfaces typed, not as a body.
        router.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn remote_pdf_fetches_then_serves_from_cache() {
        let dir = tempfile::tempdir().unwrap();
        let st = state(dir.path());
        let meta: PaperMetadata = serde_json::from_value(json!({
            "source_id": "arxiv:2204.12985",
            "version": 1,
            "title": "T",
            "authors": ["A"],
            "published": "2024-01-01",
            "summary": "S",
        }))
        .unwrap();
        st.with_conn(|conn| svc_paper::save_paper_metadata(conn, &meta, None))
            .unwrap();
        let bytes: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
        let served = dir.path().join(pdf_on_disk_name("arxiv:2204.12985", 1));
        std::fs::write(&served, &bytes).unwrap();

        let ep = client().await;
        let (router, addr) = serve(
            st,
            vec![Member {
                id: ep.id().to_string(),
                role: Role::Read,
            }],
        )
        .await;
        let remote = RemoteState::default();
        let cache = tempfile::tempdir().unwrap();
        let path = fetch_remote_pdf(
            &ep,
            &remote,
            "b1",
            addr.clone(),
            "arxiv:2204.12985",
            Some(1),
            cache.path(),
        )
        .await
        .unwrap();
        assert_eq!(path, cache.path().join("b1").join("arxiv_2204.12985v1.pdf"));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        // Mutate the node's file: a second call must hit the cache, not refetch.
        std::fs::write(&served, b"CHANGED").unwrap();
        let again = fetch_remote_pdf(
            &ep,
            &remote,
            "b1",
            addr.clone(),
            "arxiv:2204.12985",
            Some(1),
            cache.path(),
        )
        .await
        .unwrap();
        assert_eq!(again, path);
        assert_eq!(std::fs::read(&again).unwrap(), bytes);
        // A missing paper's answered error surfaces as a typed remote 404.
        let err = fetch_remote_pdf(&ep, &remote, "b1", addr, "nope", None, cache.path())
            .await
            .unwrap_err();
        assert!(
            matches!(err, RemoteError::Remote { status: 404, .. }),
            "{err:?}"
        );
        router.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn non_admitted_device_gets_one_honest_unreachable() {
        let dir = tempfile::tempdir().unwrap();
        let (router, addr) = serve(state(dir.path()), vec![]).await;
        let ep = client().await;
        let remote = RemoteState::default();
        let req = json!({ "method": "GET", "path": "/api/stats", "body": Value::Null });
        let err = request_remote(&ep, &remote, "b1", addr, &req)
            .await
            .unwrap_err();
        assert!(matches!(err, RemoteError::Unreachable { .. }), "{err:?}");
        router.shutdown().await.unwrap();
    }
}
