//! Remote Query Mode, node half (CONTEXT.md: Remote Query Mode / Member / Node
//! Address / Provider Access): the Member List with roles, the role gate enforced
//! BEFORE `route()`, and the `linxiv-api/1` handler on the share node's endpoint.
//! Only the headless bin wires this up; the desktop app never serves the ALPN.
//!
//! The Member List governs two doors at once (a known coupling): presence on
//! the list grants relay admission — any role, including `none` — while the
//! role grants query rights. Non-members and role-`none` members are refused
//! at the transport (knock logged, connection closed unanswered), so an
//! unadmitted device cannot tell "node offline" from "not admitted".

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use linxiv_p2p::{
    api, ApiHandlerFn, ApiProtocol, ApiResponse, KnockLogFn, MemberCheckFn, TransferLogFn,
    TransferOutcome,
};

use crate::route::{self, ApiRequest};
use crate::state::AppState;

/// Request cap for role `read` (design: ~1 MiB); the transport answers 413
/// past it.
pub const MAX_API_REQUEST: usize = 1024 * 1024;

/// Request cap for role `read-write`: `file_b64` PDF imports run up to
/// 100 MB, ~140 MB as base64 plus envelope headroom.
pub const MAX_API_REQUEST_RW: usize = 150 * 1024 * 1024;

/// `LINXIV_PDF_RATE_BPS` default: ~5 MB/s per member.
pub const DEFAULT_PDF_RATE_BPS: u64 = 5_000_000;

/// Per-member byte-lane pacing knob, from the environment.
pub fn pdf_rate_bps() -> u64 {
    std::env::var("LINXIV_PDF_RATE_BPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&r| r > 0)
        .unwrap_or(DEFAULT_PDF_RATE_BPS)
}

// --- member list ------------------------------------------------------------

/// A member's query role (CONTEXT.md: Member). `None` still grants relay
/// admission — presence on the list is the relay door, the role is the
/// query door.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    #[default]
    None,
    Read,
    ReadWrite,
}

impl Role {
    /// Provider Access (CONTEXT.md): may this role make the node fetch from
    /// external providers (the `route/sources.rs` groups and the feed fetch)?
    /// read-write by default, read not.
    fn provider_access(self) -> bool {
        matches!(self, Role::ReadWrite)
    }
}

/// One Member List entry: an admitted device (p2p endpoint id) + its role,
/// plus attribution — an optional display name and the journal actor ids of
/// this person's devices, paired by the operator in the admin panel.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Member {
    pub id: String,
    pub role: Role,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub actors: Vec<String>,
}

/// Wire forms: `{"id": .., "role": ..}`, or a legacy bare-string entry
/// (pre-role allowlist files), which parses as role `none` — same relay
/// admission, no query rights.
#[derive(Deserialize)]
#[serde(untagged)]
enum MemberEntry {
    Legacy(String),
    Full {
        id: String,
        #[serde(default)]
        role: Role,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        actors: Vec<String>,
    },
}

impl<'de> Deserialize<'de> for Member {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match MemberEntry::deserialize(d)? {
            MemberEntry::Legacy(id) => Member {
                id,
                ..Default::default()
            },
            MemberEntry::Full {
                id,
                role,
                name,
                actors,
            } => Member {
                id,
                role,
                name,
                actors,
            },
        })
    }
}

/// 64 hex chars — an iroh endpoint id (ed25519 public key).
pub fn valid_endpoint_id(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Journal actor id (automerge `ActorId`): non-empty even-length hex,
/// bounded so hand-entered garbage can't bloat the list.
pub fn valid_actor_hex(s: &str) -> bool {
    !s.is_empty() && s.len() % 2 == 0 && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn member_list_path() -> PathBuf {
    linxiv_core::config::data_dir().join("relay_allowlist.json")
}

/// Missing file => empty list (deny everyone). A present-but-unparseable file
/// is an error so access still denies (`unwrap_or_default`) but admin writes
/// refuse to clobber it — e.g. a botched hand-edit must not be silently
/// replaced with an empty list.
pub fn load_members() -> Result<Vec<Member>, String> {
    match std::fs::read_to_string(member_list_path()) {
        Err(_) => Ok(Vec::new()),
        Ok(s) => parse_members(&s),
    }
}

fn parse_members(s: &str) -> Result<Vec<Member>, String> {
    serde_json::from_str(s)
        .map_err(|e| format!("relay_allowlist.json is unparseable ({e}); fix or delete it"))
}

/// Sibling tmp + rename, same atomic-write pattern as `sources/download.rs`.
/// Always writes the `{id, role}` form — a legacy file upgrades on first save.
pub fn save_members(list: &[Member]) -> std::io::Result<()> {
    let path = member_list_path();
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(list).unwrap_or_default())?;
    std::fs::rename(&tmp, &path)
}

/// Relay admission: PRESENCE of any role (including `none`) allows. Empty
/// list (missing file) denies everyone — fail closed.
pub fn relay_allow(members: &[Member], endpoint_id: Option<&str>) -> bool {
    match endpoint_id {
        Some(id) if valid_endpoint_id(id) => members.iter().any(|m| m.id.eq_ignore_ascii_case(id)),
        _ => false,
    }
}

/// Query-door member check reading the on-disk Member List per connection:
/// unknown ids and role-`none` members get `None` (transport refusal).
/// Fail-closed: an unreadable/corrupt list admits nobody.
pub fn file_member_check() -> MemberCheckFn<Member> {
    Arc::new(|peer| {
        load_members()
            .unwrap_or_default()
            .into_iter()
            .find(|m| m.id.eq_ignore_ascii_case(peer) && m.role != Role::None)
    })
}

// --- enforcement ------------------------------------------------------------

/// The remote-surface gate, checked BEFORE `route()` ever runs. `None` =
/// allowed through to the router.
/// - `settings` / `storage` / `env` are operator-only: 403 for every role
///   (`PATCH /api/env` writes secrets to UserSettings and the process env).
/// - `read` is GET-only.
/// - External Provider fetches on the node's quota — the `route/sources.rs`
///   groups (`arxiv`/`openalex`/`crossref`/`doi`) and `GET /api/feed`, whose
///   `url=` makes the node fetch an arbitrary caller-supplied URL — need
///   Provider Access, which `read` lacks.
///
/// Share dispatch and `/api/admin/*` need no arm here: the handler only ever
/// calls `route()`, which routes neither (plain 404).
pub fn deny_reason(role: Role, method: &str, path: &str) -> Option<&'static str> {
    let raw_path = path.split('?').next().unwrap_or(path);
    let segs = route::split_segments(raw_path);
    let group = segs.get(1).map(String::as_str);
    if matches!(group, Some("settings") | Some("storage") | Some("env")) {
        return Some("operator-only route group");
    }
    // `pdf-path` answers the node's absolute filesystem path — operator
    // layout never crosses the remote surface, any role. Remote clients use
    // the PDF byte lane; a missing PDF is the lane's 404.
    if group == Some("papers") && segs.len() == 4 && segs[3] == "pdf-path" {
        return Some("pdf-path is local-only; use the pdf byte lane");
    }
    let node_side_fetch = matches!(
        group,
        Some("arxiv") | Some("openalex") | Some("crossref") | Some("doi")
    ) || (group == Some("feed") && segs.len() == 2);
    if node_side_fetch && !role.provider_access() {
        return Some("route requires Provider Access");
    }
    match role {
        // Unreachable in production (the member check refuses `none` at the
        // transport); kept fail-closed for any future caller.
        Role::None => Some("role grants no query access"),
        Role::Read if method != "GET" => Some("read role is GET-only"),
        _ => None,
    }
}

// --- transfer log -----------------------------------------------------------

const TRANSFER_LOG_CAP: usize = 200;

/// Recent byte-lane outcomes — the sender's own view of each transfer (there
/// is no application-level receipt by design).
/// ponytail: in-memory only, resets on restart; persist if audit matters.
#[derive(Default)]
pub struct TransferLog {
    seq: u64,
    entries: VecDeque<Value>,
}

impl TransferLog {
    pub fn push(&mut self, endpoint_id: &str, outcome: TransferOutcome) {
        self.seq += 1;
        if self.entries.len() >= TRANSFER_LOG_CAP {
            self.entries.pop_front();
        }
        let (outcome_s, bytes) = match outcome {
            TransferOutcome::Delivered { bytes } => ("delivered", bytes),
            TransferOutcome::Aborted { sent } => ("aborted", sent),
        };
        self.entries.push_back(json!({
            "seq": self.seq,
            "endpoint_id": endpoint_id,
            "outcome": outcome_s,
            "bytes": bytes,
        }));
    }

    pub fn entries(&self) -> &VecDeque<Value> {
        &self.entries
    }
}

// --- protocol handler -------------------------------------------------------

/// Builds the `linxiv-api/1` handler: member gate at the transport, the role
/// gate in front of `route()`, and the PDF byte lane. `transfer_log` fires
/// exactly once per byte-lane answer; the builder wraps it to also close the
/// per-member rate bookkeeping.
pub fn build_api_proto(
    state: Arc<AppState>,
    member_check: MemberCheckFn<Member>,
    knock_log: KnockLogFn,
    transfer_log: TransferLogFn,
    rate_bps: u64,
) -> ApiProtocol<Member> {
    // Active byte-lane transfers per endpoint id: a member's rate is split
    // across their concurrent streams at admission time.
    // ponytail: rate/active split is fixed at stream start; a true shared
    // token bucket only if mid-transfer fairness ever matters.
    let active: Arc<Mutex<HashMap<String, u64>>> = Default::default();
    let active_dec = active.clone();
    let transfer_log: TransferLogFn = Arc::new(move |peer: &str, outcome| {
        // Endpoint ids compare case-insensitively; the increment keys by the
        // Member List's stored casing, this decrement by the transport's —
        // lowercase both so the bookkeeping always matches.
        let key = peer.to_ascii_lowercase();
        let mut a = active_dec.lock().unwrap();
        if let Some(n) = a.get_mut(&key) {
            *n -= 1;
            if *n == 0 {
                a.remove(&key);
            }
        }
        drop(a);
        transfer_log(peer, outcome);
    });
    let handler: ApiHandlerFn<Member> = Arc::new(move |member: Member, body: Vec<u8>| {
        let state = state.clone();
        let active = active.clone();
        Box::pin(async move { handle(&state, member, &body, rate_bps, &active).await })
    });
    ApiProtocol::new(
        member_check,
        knock_log,
        transfer_log,
        handler,
        Arc::new(|m: &Member| match m.role {
            Role::ReadWrite => MAX_API_REQUEST_RW,
            _ => MAX_API_REQUEST,
        }),
    )
}

fn err_env(status: u16, detail: &str) -> Value {
    json!({ "status": status, "detail": detail })
}

async fn handle(
    state: &Arc<AppState>,
    member: Member,
    body: &[u8],
    rate_bps: u64,
    active: &Arc<Mutex<HashMap<String, u64>>>,
) -> ApiResponse {
    let req: ApiRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return ApiResponse::Json(err_env(400, &format!("invalid request: {e}"))),
    };
    if let Some(reason) = deny_reason(member.role, &req.method, &req.path) {
        return ApiResponse::Json(err_env(403, reason));
    }
    // PDF byte lane: `GET /api/papers/{id}/pdf?version=`, resolved via the
    // same path the `pdf-path` arm uses. Errors answer as a bare envelope
    // (no newline, no bytes) — the client's byte-lane reader handles both.
    let (raw_path, raw_query) = req.path.split_once('?').unwrap_or((req.path.as_str(), ""));
    let segs = route::split_segments(raw_path);
    if req.method == "GET" {
        if let ["api", "papers", sid, "pdf"] = segs
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice()
        {
            return pdf_lane(state, sid, raw_query, &member.id, rate_bps, active).await;
        }
    }
    // Mutating requests journal their delta under the member's endpoint id
    // (a valid automerge actor), so remote writes are attributed to the
    // authenticated member — no manual actor pairing needed for them.
    let result = if req.method == "GET" {
        route::route(state, req).await
    } else {
        let actor = member.id.to_ascii_lowercase();
        crate::journal::attribute_remote_write(
            state,
            &crate::journal::journal_dir(),
            &actor,
            || route::route(state, req),
        )
        .await
    };
    ApiResponse::Json(match result {
        Ok(body) => json!({ "status": 200, "body": body }),
        Err(e) => err_env(e.status, &e.detail),
    })
}

async fn pdf_lane(
    state: &Arc<AppState>,
    source_id: &str,
    raw_query: &str,
    peer: &str,
    rate_bps: u64,
    active: &Arc<Mutex<HashMap<String, u64>>>,
) -> ApiResponse {
    // Same `?version=` contract as `pdf-path`: absent → latest; else int >= 1 or 422.
    let version = match route::parse_query(raw_query).get("version") {
        None => None,
        Some(v) => match v.parse::<i64>().ok().filter(|&n| n >= 1) {
            Some(n) => Some(n),
            None => return ApiResponse::Json(err_env(422, "version must be an integer >= 1")),
        },
    };
    let path = match route::pdfs::resolve_pdf(state, source_id, version) {
        Ok((_, _, path)) => path,
        Err(e) => return ApiResponse::Json(err_env(e.status, &e.detail)),
    };
    let (file, size) = match tokio::fs::File::open(&path).await {
        Ok(f) => match f.metadata().await {
            Ok(m) => (f, m.len()),
            Err(e) => return ApiResponse::Json(err_env(500, &format!("stat pdf: {e}"))),
        },
        Err(e) => return ApiResponse::Json(err_env(500, &format!("open pdf: {e}"))),
    };
    // Admit the transfer: split the member's rate across their concurrent
    // streams. The wrapped transfer_log decrements — it fires exactly once
    // for every Bytes response the transport takes from here.
    let rate = {
        let mut a = active.lock().unwrap();
        // Lowercased to match the transfer_log decrement's key.
        let n = a.entry(peer.to_ascii_lowercase()).or_insert(0);
        *n += 1;
        (rate_bps / *n).max(1)
    };
    ApiResponse::Bytes {
        header: api::byte_header(size, rate),
        source: Box::new(file),
        size,
        rate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(id: &str, role: Role) -> Member {
        Member {
            id: id.into(),
            role,
            ..Default::default()
        }
    }

    #[test]
    fn member_list_parses_legacy_new_and_mixed_entries() {
        let id_a = "aa".repeat(32);
        let id_b = "bb".repeat(32);
        // Legacy bare strings => role none.
        let legacy: Vec<Member> = serde_json::from_str(&format!(r#"["{id_a}","{id_b}"]"#)).unwrap();
        assert_eq!(legacy, vec![m(&id_a, Role::None), m(&id_b, Role::None)]);
        // New objects, role optional (defaults to none), mixed with legacy.
        let mixed: Vec<Member> = serde_json::from_str(&format!(
            r#"[{{"id":"{id_a}","role":"read-write"}},{{"id":"{id_b}"}},"cc"]"#
        ))
        .unwrap();
        assert_eq!(
            mixed,
            vec![
                m(&id_a, Role::ReadWrite),
                m(&id_b, Role::None),
                m("cc", Role::None),
            ]
        );
        // Round trip is always the object form; absent name/actors stay absent.
        let json = serde_json::to_value(&mixed).unwrap();
        assert_eq!(
            json[0],
            serde_json::json!({"id": id_a, "role": "read-write"})
        );
        assert_eq!(json[2], serde_json::json!({"id": "cc", "role": "none"}));
        // Attribution fields parse and round trip.
        let named: Vec<Member> = serde_json::from_str(&format!(
            r#"[{{"id":"{id_a}","role":"read","name":"Ada","actors":["dd{}"]}}]"#,
            "ee".repeat(15)
        ))
        .unwrap();
        assert_eq!(named[0].name.as_deref(), Some("Ada"));
        assert_eq!(named[0].actors.len(), 1);
        let json = serde_json::to_value(&named).unwrap();
        assert_eq!(json[0]["name"], "Ada");
        assert!(json[0]["actors"].is_array());
    }

    #[test]
    fn actor_hex_validation() {
        assert!(valid_actor_hex(&"ab".repeat(16))); // random automerge actor
        assert!(valid_actor_hex("00ff"));
        assert!(!valid_actor_hex(""));
        assert!(!valid_actor_hex("abc")); // odd length
        assert!(!valid_actor_hex("zz"));
        assert!(!valid_actor_hex(&"ab".repeat(65))); // too long
    }

    #[test]
    fn corrupt_member_list_is_an_error_not_an_empty_list() {
        assert!(parse_members("{ not json").is_err());
        assert!(parse_members(r#"[{"role":"read"}]"#).is_err()); // no id
        assert!(parse_members(r#"[{"id":"aa","role":"admin"}]"#).is_err()); // bad role
    }

    #[test]
    fn relay_allow_is_presence_based_and_fail_closed() {
        let id = "ab".repeat(32);
        // Any role admits to the relay — including none.
        for role in [Role::None, Role::Read, Role::ReadWrite] {
            assert!(relay_allow(&[m(&id, role)], Some(&id)));
        }
        assert!(relay_allow(&[m(&id, Role::None)], Some(&id.to_uppercase())));
        // Missing file => empty list => deny everyone.
        assert!(!relay_allow(&[], Some(&id)));
        // Absent or malformed id => deny.
        let list = vec![m(&id, Role::Read)];
        assert!(!relay_allow(&list, None));
        assert!(!relay_allow(&list, Some("")));
        assert!(!relay_allow(&list, Some("not-hex-garbage")));
        assert!(!relay_allow(&list, Some(&id[..40]))); // too short
    }

    /// The full role × surface matrix `deny_reason` enforces.
    #[test]
    fn enforcement_matrix() {
        let ok = |role, method, path| assert_eq!(deny_reason(role, method, path), None);
        let denied = |role, method, path| assert!(deny_reason(role, method, path).is_some());
        // Excluded groups: operator-only for EVERY role. pdf-path leaks the
        // node's filesystem layout — remote clients use the byte lane.
        for role in [Role::None, Role::Read, Role::ReadWrite] {
            denied(role, "GET", "/api/settings");
            denied(role, "PATCH", "/api/settings");
            denied(role, "PATCH", "/api/env");
            denied(role, "GET", "/api/storage/info");
            denied(role, "GET", "/api/papers/2204.12985/pdf-path");
            denied(role, "GET", "/api/papers/2204.12985/pdf-path?version=2");
        }
        // read: GET only, no Provider Access (provider groups + the feed
        // fetch, whose url= is a node-side fetch of an arbitrary URL).
        ok(Role::Read, "GET", "/api/papers");
        ok(Role::Read, "GET", "/api/papers/2204.12985/pdf?version=2");
        ok(Role::Read, "GET", "/api/feed/rules");
        denied(Role::Read, "POST", "/api/projects");
        denied(Role::Read, "DELETE", "/api/papers/x");
        denied(Role::Read, "GET", "/api/feed?url=http%3A%2F%2F10.0.0.5%2F");
        for group in ["arxiv", "openalex", "crossref", "doi"] {
            let path = format!("/api/{group}/anything");
            assert!(deny_reason(Role::Read, "GET", &path).is_some(), "{path}");
        }
        // read-write: data-plane writes + provider fetches, still no
        // operator groups.
        ok(Role::ReadWrite, "POST", "/api/projects");
        ok(Role::ReadWrite, "POST", "/api/arxiv/search");
        ok(Role::ReadWrite, "GET", "/api/feed?url=http%3A%2F%2Fx%2F");
        // none: nothing (belt — the transport refuses it first).
        denied(Role::None, "GET", "/api/papers");
    }
}

/// In-process protocol round trips: this crate's node handler served over the
/// real transport, driven with the p2p crate's client — the same pairing the
/// headless bin and a remote app will run.
#[cfg(test)]
mod proto_tests {
    use super::*;
    use iroh::{endpoint::presets, protocol::Router, Endpoint};
    use linxiv_core::models::PaperMetadata;
    use linxiv_core::service::paper as svc_paper;
    use serde_json::json;

    const RATE: u64 = 1_000_000;

    fn state(pdf_dir: &std::path::Path) -> Arc<AppState> {
        let conn = linxiv_core::storage::open_in_memory().unwrap();
        linxiv_core::storage::init_db(&conn).unwrap();
        Arc::new(AppState::from_parts(
            conn,
            pdf_dir.to_path_buf(),
            std::env::temp_dir(),
        ))
    }

    struct Node {
        router: Router,
        addr: iroh::EndpointAddr,
        knocks: Arc<Mutex<Vec<String>>>,
        transfers: Arc<Mutex<TransferLog>>,
    }

    /// Serve `state` behind an injected member map (never the on-disk list).
    async fn serve(state: Arc<AppState>, members: Vec<Member>) -> Node {
        let knocks: Arc<Mutex<Vec<String>>> = Default::default();
        let transfers: Arc<Mutex<TransferLog>> = Default::default();
        let member_check: MemberCheckFn<Member> = Arc::new(move |peer| {
            members
                .iter()
                .find(|m| m.id.eq_ignore_ascii_case(peer) && m.role != Role::None)
                .cloned()
        });
        let k = knocks.clone();
        let t = transfers.clone();
        let proto = build_api_proto(
            state,
            member_check,
            Arc::new(move |peer: &str| k.lock().unwrap().push(peer.to_string())),
            Arc::new(move |peer: &str, outcome| t.lock().unwrap().push(peer, outcome)),
            RATE,
        );
        let server = Endpoint::builder(presets::Minimal).bind().await.unwrap();
        let router = Router::builder(server).accept(api::ALPN, proto).spawn();
        let addr = router.endpoint().addr();
        Node {
            router,
            addr,
            knocks,
            transfers,
        }
    }

    async fn client() -> Endpoint {
        Endpoint::builder(presets::Minimal).bind().await.unwrap()
    }

    fn req(method: &str, path: &str, body: Value) -> Value {
        json!({ "method": method, "path": path, "body": body })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn unknown_id_refused_at_transport_and_knock_logged() {
        let dir = tempfile::tempdir().unwrap();
        let node = serve(state(dir.path()), vec![]).await;
        let ep = client().await;
        let conn = api::connect(&ep, node.addr.clone()).await.unwrap();
        let err = api::request(&conn, &req("GET", "/api/papers", Value::Null))
            .await
            .expect_err("stranger must get no answer");
        assert!(matches!(err, linxiv_p2p::ApiClientError::Refused));
        assert_eq!(*node.knocks.lock().unwrap(), vec![ep.id().to_string()]);
        node.router.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn read_member_is_get_only_and_gated() {
        let dir = tempfile::tempdir().unwrap();
        let ep = client().await;
        let node = serve(
            state(dir.path()),
            vec![Member {
                id: ep.id().to_string(),
                role: Role::Read,
                ..Default::default()
            }],
        )
        .await;
        let conn = api::connect(&ep, node.addr.clone()).await.unwrap();
        // GET papers: through the gate, answered by route().
        let env = api::request(&conn, &req("GET", "/api/papers", Value::Null))
            .await
            .unwrap();
        assert_eq!(env["status"], 200, "got {env}");
        assert!(env["body"]["papers"].is_array() || env["body"].is_object());
        // Writes, operator groups and provider fetches: 403 before route()
        // runs (no network touched).
        for (method, path) in [
            ("POST", "/api/projects"),
            ("GET", "/api/settings"),
            ("PATCH", "/api/env"),
            ("GET", "/api/storage/info"),
            ("GET", "/api/arxiv/search"),
            ("GET", "/api/feed?url=http%3A%2F%2F10.0.0.5%2F"),
            ("GET", "/api/papers/2204.12985/pdf-path"),
        ] {
            let env = api::request(&conn, &req(method, path, json!({ "name": "P" })))
                .await
                .unwrap();
            assert_eq!(env["status"], 403, "{method} {path}: {env}");
        }
        assert!(node.knocks.lock().unwrap().is_empty());
        node.router.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn read_write_member_can_write_and_reach_sources() {
        let dir = tempfile::tempdir().unwrap();
        let ep = client().await;
        let node = serve(
            state(dir.path()),
            vec![Member {
                id: ep.id().to_string(),
                role: Role::ReadWrite,
                ..Default::default()
            }],
        )
        .await;
        let conn = api::connect(&ep, node.addr.clone()).await.unwrap();
        let env = api::request(&conn, &req("POST", "/api/projects", json!({ "name": "P" })))
            .await
            .unwrap();
        assert_eq!(env["status"], 200, "got {env}");
        // Past the Provider Access gate: an unrouted provider path is a plain
        // 404 from route(), not the gate's 403 (no network touched).
        let env = api::request(&conn, &req("GET", "/api/arxiv/nope", Value::Null))
            .await
            .unwrap();
        assert_eq!(env["status"], 404, "got {env}");
        // Operator groups stay closed even for read-write.
        for (method, path) in [("PATCH", "/api/settings"), ("PATCH", "/api/env")] {
            let env = api::request(&conn, &req(method, path, json!({})))
                .await
                .unwrap();
            assert_eq!(env["status"], 403, "{method} {path}: {env}");
        }
        node.router.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn request_cap_is_role_aware() {
        let dir = tempfile::tempdir().unwrap();
        // ~2 MiB body: past the read cap, well under the read-write cap.
        let big = json!({ "pad": "x".repeat(2 * 1024 * 1024) });
        // read: the transport answers 413 before any routing.
        let ep = client().await;
        let node = serve(
            state(dir.path()),
            vec![Member {
                id: ep.id().to_string(),
                role: Role::Read,
                ..Default::default()
            }],
        )
        .await;
        let conn = api::connect(&ep, node.addr.clone()).await.unwrap();
        let env = api::request(&conn, &req("GET", "/api/papers", big.clone()))
            .await
            .unwrap();
        assert_eq!(env["status"], 413, "got {env}");
        node.router.shutdown().await.unwrap();
        // read-write: the same body clears the cap — an unrouted path is
        // route()'s 404, proving the request was read and dispatched.
        let ep = client().await;
        let node = serve(
            state(dir.path()),
            vec![Member {
                id: ep.id().to_string(),
                role: Role::ReadWrite,
                ..Default::default()
            }],
        )
        .await;
        let conn = api::connect(&ep, node.addr.clone()).await.unwrap();
        let env = api::request(&conn, &req("POST", "/api/nope", big))
            .await
            .unwrap();
        assert_eq!(env["status"], 404, "got {env}");
        node.router.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn pdf_lane_delivers_exact_bytes_and_logs_the_transfer() {
        let dir = tempfile::tempdir().unwrap();
        let st = state(dir.path());
        // A saved paper whose managed PDF exists on disk.
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
        let bytes: Vec<u8> = (0..30_000u32).map(|i| (i % 251) as u8).collect();
        let name = svc_paper::pdf_on_disk_name("arxiv:2204.12985", 1);
        std::fs::write(dir.path().join(name), &bytes).unwrap();

        let ep = client().await;
        let node = serve(
            st,
            vec![Member {
                id: ep.id().to_string(),
                role: Role::Read, // GET: the lane is open to readers
                ..Default::default()
            }],
        )
        .await;
        let conn = api::connect(&ep, node.addr.clone()).await.unwrap();
        let (header, lane) = api::request_bytes(
            &conn,
            &req("GET", "/api/papers/arxiv%3A2204.12985/pdf", Value::Null),
        )
        .await
        .unwrap();
        assert_eq!(header["status"], 200, "got {header}");
        assert_eq!(header["size"], bytes.len() as u64);
        assert_eq!(header["rate"], RATE);
        assert!(header["eta_seconds"].as_f64().unwrap() > 2.0);
        assert_eq!(lane.size(), bytes.len() as u64);
        assert_eq!(lane.read_to_vec().await.unwrap(), bytes);
        // The sender logs Delivered after its own finish(), which races our
        // read completing; poll briefly like the p2p crate's lane test.
        for _ in 0..50 {
            if !node.transfers.lock().unwrap().entries().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        {
            let transfers = node.transfers.lock().unwrap();
            let entry = transfers.entries().back().expect("a logged transfer");
            assert_eq!(entry["endpoint_id"], ep.id().to_string());
            assert_eq!(entry["outcome"], "delivered");
            assert_eq!(entry["bytes"], bytes.len() as u64);
        }

        // Error case: bare error envelope, no bytes, nothing logged.
        let (header, lane) =
            api::request_bytes(&conn, &req("GET", "/api/papers/nope/pdf", Value::Null))
                .await
                .unwrap();
        assert_eq!(header["status"], 404, "got {header}");
        assert_eq!(lane.size(), 0);
        assert_eq!(node.transfers.lock().unwrap().entries().len(), 1);
        node.router.shutdown().await.unwrap();
    }

    /// The stream accounting increments by the Member List's stored casing
    /// and decrements by the transport's: with an UPPERCASE list entry the
    /// count must still return to zero, so a second transfer gets the full
    /// rate instead of a forever-shrunk share.
    #[tokio::test(flavor = "multi_thread")]
    async fn pdf_lane_bookkeeping_survives_member_id_casing() {
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
        let name = svc_paper::pdf_on_disk_name("arxiv:2204.12985", 1);
        std::fs::write(dir.path().join(name), b"%PDF-tiny").unwrap();

        let ep = client().await;
        let node = serve(
            st,
            vec![Member {
                id: ep.id().to_string().to_ascii_uppercase(),
                role: Role::Read,
                ..Default::default()
            }],
        )
        .await;
        let conn = api::connect(&ep, node.addr.clone()).await.unwrap();
        for done in 0..2u64 {
            // Wait for the previous transfer's decrement (the wrapped
            // transfer_log decrements before it logs) so admissions never
            // legitimately overlap.
            for _ in 0..50 {
                if node.transfers.lock().unwrap().entries().len() as u64 == done {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            let (header, lane) = api::request_bytes(
                &conn,
                &req("GET", "/api/papers/arxiv%3A2204.12985/pdf", Value::Null),
            )
            .await
            .unwrap();
            assert_eq!(header["status"], 200, "got {header}");
            assert_eq!(header["rate"], RATE, "transfer {done}: count leaked");
            lane.read_to_vec().await.unwrap();
        }
        node.router.shutdown().await.unwrap();
    }
}
