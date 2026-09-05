//! Headless linXiv node (containerized / self-hosted, TODO.md
//! "containerization"): the full `/api/*` router over HTTP — share routes
//! included — plus the iroh share node and the 5-minute background sync, no
//! Tauri window. Same dispatch surface as the app; the dev_server bin stays
//! the dev-loop shim.
//!
//! Auth: `LINXIV_API_TOKEN` gates every request behind `Authorization:
//! Bearer <token>`. Fail-closed: binding a non-loopback `LINXIV_HTTP_ADDR`
//! without a token refuses to start (the container image binds `0.0.0.0:8000`,
//! so it always requires one); loopback without a token stays open for the
//! local dev loop. Relay settings are the same on-disk user settings as the
//! app (`p2p_relay_url` / `p2p_relay_auth_token` / `p2p_relay_only`): set
//! them via `PATCH /api/settings`, then `POST /api/share/relay/reconnect`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::any,
    Router,
};

use linxiv_server::remote_query::{
    self, load_members, relay_allow, save_members, valid_endpoint_id, Member, Role, TransferLog,
};
use linxiv_server::route::share::ShareState;
use linxiv_server::route::{feed, route, share, ApiRequest};
use linxiv_server::state::AppState;
use linxiv_server::{full_text_worker, p2p_config, share_sync};

/// Base64 file uploads ride the JSON body, so allow a large request body.
const MAX_BODY: usize = 200 * 1024 * 1024;

/// Static admin page. Secretless, so served without auth at `GET /admin`;
/// every API call it makes carries the bearer token.
const ADMIN_HTML: &str = include_str!("../headless_admin.html");

#[derive(Clone)]
struct Ctx {
    state: Arc<AppState>,
    share: Arc<ShareState>,
    /// Bearer token every request must present; `None` only on loopback.
    token: Option<Arc<str>>,
    /// Process start, for `uptime_secs` in `GET /api/status`.
    started: Instant,
    /// Relay access log + serialization of member-list file writes.
    /// ponytail: one Mutex around list+log; split if relay checks ever contend.
    relay: Arc<Mutex<RelayLog>>,
    /// Byte-lane transfer outcomes (Remote Query Mode PDF lane).
    transfers: Arc<Mutex<TransferLog>>,
}

#[tokio::main]
async fn main() {
    let started = Instant::now();
    let data_dir = linxiv_core::config::init_data_dir().expect("init data dir");
    eprintln!("linxiv headless: data dir {}", data_dir.display());
    let state = Arc::new(AppState::new().expect("init app state"));
    // Keychain access is sync (and absent in containers, where the
    // LINXIV_P2P_PASSPHRASE fallback applies) — keep it off the async runtime.
    let dek = tokio::task::spawn_blocking(p2p_config::p2p_dek)
        .await
        .expect("resolve p2p dek");
    let (share_state, node_bound) = share::startup_share_state(dek)
        .await
        .expect("init share state");
    let addr = std::env::var("LINXIV_HTTP_ADDR").unwrap_or_else(|_| "127.0.0.1:8000".into());
    let token: Option<Arc<str>> = std::env::var("LINXIV_API_TOKEN")
        .ok()
        .filter(|t| !t.is_empty())
        .map(Into::into);
    // Fail closed: an unauthenticated API is only acceptable on loopback.
    // An unparseable addr (e.g. a hostname) counts as non-loopback.
    let loopback = addr
        .parse::<std::net::SocketAddr>()
        .is_ok_and(|a| a.ip().is_loopback());
    if token.is_none() && !loopback {
        eprintln!(
            "error: LINXIV_HTTP_ADDR={addr} is not loopback and LINXIV_API_TOKEN is unset; \
             refusing to serve an unauthenticated API beyond localhost"
        );
        std::process::exit(1);
    }

    let ctx = Ctx {
        state,
        share: Arc::new(share_state),
        token,
        started,
        relay: Arc::new(Mutex::new(RelayLog::open(
            data_dir.join("relay_access_log.jsonl"),
        ))),
        transfers: Arc::new(Mutex::new(TransferLog::default())),
    };
    if node_bound && ctx.share.mark_sync_started() {
        spawn_interval_sync(&ctx);
    }
    install_remote_query(&ctx).await;
    // Idles until `full_text_worker_enabled` is switched on, same as the app.
    full_text_worker::spawn_headless(ctx.state.clone());
    spawn_feed_poll(ctx.state.clone());
    // An always-on node on a laptop/desktop must not suspend out from under
    // its peers; the fd releases itself on process exit, crash included.
    #[cfg(target_os = "linux")]
    let _sleep_inhibitor = inhibit_sleep().await;

    let share = ctx.share.clone();
    let auth = if ctx.token.is_some() {
        "bearer auth"
    } else {
        "UNAUTHENTICATED (loopback)"
    };
    let app = Router::new().fallback(any(dispatch)).with_state(ctx);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("bind headless server");
    eprintln!("linxiv headless on http://{addr} (node bound: {node_bound}, {auth})");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("headless serve");
    // Close the iroh endpoint + router explicitly — Drop is not enough.
    if let Err(e) = share.shutdown().await {
        eprintln!("warning: share node shutdown: {e}");
    }
}

/// Take a systemd-logind sleep+idle inhibitor for the process lifetime, so
/// the machine running the node stays reachable by default. Opt out with
/// `LINXIV_ALLOW_SLEEP=1`. The lock is a pipe fd — logind releases it when
/// this process exits, however it exits. Where login1 is absent (containers,
/// non-systemd hosts) this degrades to one stderr line.
#[cfg(target_os = "linux")]
async fn inhibit_sleep() -> Option<zbus::zvariant::OwnedFd> {
    if std::env::var("LINXIV_ALLOW_SLEEP").as_deref() == Ok("1") {
        eprintln!("linxiv headless: LINXIV_ALLOW_SLEEP=1 set; system sleep settings apply");
        return None;
    }
    let take = async {
        let conn = zbus::Connection::system().await?;
        let reply = conn
            .call_method(
                Some("org.freedesktop.login1"),
                "/org/freedesktop/login1",
                Some("org.freedesktop.login1.Manager"),
                "Inhibit",
                &(
                    "sleep:idle",
                    "linxiv-headless",
                    "serving the linXiv API and p2p node",
                    "block",
                ),
            )
            .await?;
        reply.body().deserialize::<zbus::zvariant::OwnedFd>()
    };
    // A wedged (present but unresponsive) system bus must not block startup.
    match tokio::time::timeout(Duration::from_secs(10), take).await {
        Ok(Ok(fd)) => {
            eprintln!("linxiv headless: sleep/idle inhibited while running (LINXIV_ALLOW_SLEEP=1 to opt out)");
            Some(fd)
        }
        Ok(Err(e)) => {
            eprintln!(
                "linxiv headless: sleep inhibit unavailable ({e}); system sleep settings apply"
            );
            None
        }
        Err(_) => {
            eprintln!("linxiv headless: sleep inhibit timed out; system sleep settings apply");
            None
        }
    }
}

/// Resolves on SIGTERM (docker/podman stop — this bin is PID 1, which gets no
/// default signal handling) or ctrl-c.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c()
        .await
        .expect("install ctrl-c handler");
    eprintln!("linxiv headless: shutting down");
}

/// Constant-time byte comparison (length still leaks; that's standard for
/// bearer tokens).
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// `Some(response)` when the request must be rejected.
fn check_auth(ctx: &Ctx, req: &Request) -> Option<Response> {
    let Some(token) = &ctx.token else { return None };
    let presented = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    match presented {
        Some(p) if ct_eq(p.as_bytes(), token.as_bytes()) => None,
        _ => Some(json(
            StatusCode::UNAUTHORIZED,
            &serde_json::json!({ "detail": "missing or invalid bearer token" }),
        )),
    }
}

/// Remote Query Mode: serve `linxiv-api/1` on the share node's endpoint.
/// Registered through `install_api` so a relay-reconnect rebind re-applies
/// the handler; knocks land in the relay/access log with `source: "api"`.
async fn install_remote_query(ctx: &Ctx) {
    let relay_log = ctx.relay.clone();
    let knock: linxiv_p2p::KnockLogFn = Arc::new(move |peer: &str| {
        relay_log.lock().unwrap().push(Some(peer), false, "api");
    });
    let transfers = ctx.transfers.clone();
    let transfer: linxiv_p2p::TransferLogFn = Arc::new(move |peer: &str, outcome| {
        transfers.lock().unwrap().push(peer, outcome);
    });
    let proto = remote_query::build_api_proto(
        ctx.state.clone(),
        remote_query::file_member_check(),
        knock,
        transfer,
        remote_query::pdf_rate_bps(),
    );
    ctx.share
        .install_api(Arc::new(move |node| {
            node.set_api_protocol(Box::new(proto.clone()));
        }))
        .await;
}

/// Same 5-minute loop the app spawns, minus the `AppHandle`.
fn spawn_interval_sync(ctx: &Ctx) {
    let (state, share) = (ctx.state.clone(), ctx.share.clone());
    tokio::spawn(async move {
        loop {
            share_sync::sync_all(&state, &share).await;
            share_sync::next_sync_due().await;
        }
    });
}

/// The desktop app refreshes the home feed when the user opens the screen; an
/// always-on node polls instead. No-op while `home_feed_url` is unset — the
/// settings read each tick is a small JSON file. Cadence comes from the
/// `headless_feed_poll_minutes` setting (default 30), re-read every tick so a
/// `PATCH /api/settings` takes effect on the next tick without a restart.
const FEED_POLL_DEFAULT: Duration = Duration::from_secs(30 * 60);

/// `headless_feed_poll_minutes` → sleep duration. Missing / non-positive /
/// non-integer / overflowing falls back to the default rather than a hot loop.
fn feed_poll_period(minutes: Option<i64>) -> Duration {
    minutes
        .filter(|&m| m > 0)
        .and_then(|m| (m as u64).checked_mul(60))
        .map(Duration::from_secs)
        .unwrap_or(FEED_POLL_DEFAULT)
}

fn spawn_feed_poll(state: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            let mut period = FEED_POLL_DEFAULT;
            match linxiv_core::config::UserSettings::load() {
                Ok(s) => {
                    period = feed_poll_period(
                        s.get("headless_feed_poll_minutes").and_then(|v| v.as_i64()),
                    );
                    let url = s
                        .get("home_feed_url")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|u| !u.is_empty())
                        .map(String::from);
                    if let Some(url) = url {
                        let days = s.rss_cache_retention_days();
                        if let Err(e) = feed::refresh(&state, &url, days).await {
                            eprintln!("feed poll {url}: {} {}", e.status, e.detail);
                        }
                    }
                }
                Err(e) => eprintln!("feed poll: settings unreadable: {e}"),
            }
            tokio::time::sleep(period).await;
        }
    });
}

// --- Relay access control + Member List ------------------------------------
// iroh-relay's `access.http` POSTs `/api/relay-access` with an
// `X-Iroh-NodeId` header per connecting endpoint (that exact name — the 1.0.2
// source's X_IROH_ENDPOINT_ID const is "X-Iroh-NodeId"; we also accept
// `X-Iroh-Endpoint-Id`, the name the docs use, in case a later release renames
// it). Only an exact 200 `true` (text/plain) allows. Decision source: the
// Member List at `<data_dir>/relay_allowlist.json` (`remote_query::Member` —
// `{id, role}`, legacy bare strings = role none) — missing/empty file denies
// everyone. Relay admission is presence-based; the role only governs Remote
// Query Mode rights.

const RELAY_LOG_CAP: usize = 200;

/// Rotate the on-disk audit file past this size (one `.1` generation kept),
/// so a hammered public node can't grow it without bound.
const RELAY_LOG_FILE_CAP: u64 = 5 * 1024 * 1024;

/// Recent relay access decisions plus refused api knocks (`source` tells
/// them apart: "relay" vs "api"). Appended as JSONL under the data dir so the
/// audit trail survives restarts; the in-memory tail serves the admin route.
struct RelayLog {
    seq: u64,
    entries: VecDeque<serde_json::Value>,
    /// Append handle; `None` when the file can't be opened (warned once at
    /// startup) — the in-memory log keeps working.
    file: Option<std::fs::File>,
    path: std::path::PathBuf,
}

/// Knock ids are attacker-controlled bytes (a real endpoint id is 64 hex
/// chars): strip control characters and clamp the length before a log entry
/// stores — and an admin page or terminal later renders — them.
fn clean_log_id(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(128).collect()
}

impl RelayLog {
    /// Open (or create) the JSONL audit file and seed the in-memory tail +
    /// `seq` from it, so restarts continue the sequence instead of resetting.
    fn open(path: std::path::PathBuf) -> Self {
        let mut seq = 0;
        let mut entries = VecDeque::new();
        // Rotated generation first, so a restart right after rotation still
        // shows the recent tail, not just the few post-rotation entries.
        let rotated = std::fs::read_to_string(path.with_extension("jsonl.1")).unwrap_or_default();
        let current = std::fs::read_to_string(&path).unwrap_or_default();
        for v in rotated
            .lines()
            .chain(current.lines())
            .filter_map(|l| serde_json::from_str(l).ok())
        {
            let v: serde_json::Value = v;
            seq = seq.max(v["seq"].as_u64().unwrap_or(0));
            if entries.len() >= RELAY_LOG_CAP {
                entries.pop_front();
            }
            entries.push_back(v);
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| eprintln!("warning: access log {} unwritable: {e}", path.display()))
            .ok();
        // A crash mid-write leaves a torn final line; terminate it so the
        // next entry doesn't merge into it and take both down at parse time.
        if !current.is_empty() && !current.ends_with('\n') {
            if let Some(f) = &mut file {
                use std::io::Write;
                let _ = f.write_all(b"\n");
            }
        }
        Self {
            seq,
            entries,
            file,
            path,
        }
    }

    fn push(&mut self, endpoint_id: Option<&str>, allowed: bool, source: &str) {
        self.seq += 1;
        let entry = serde_json::json!({
            "seq": self.seq,
            "at": chrono::Utc::now().to_rfc3339(),
            "endpoint_id": endpoint_id.map(clean_log_id),
            "allowed": allowed,
            "source": source,
        });
        if self
            .file
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .is_some_and(|m| m.len() > RELAY_LOG_FILE_CAP)
        {
            // A failed rename would regrow the same file forever; dropping
            // the handle keeps disk bounded and the warning one-time.
            self.file = std::fs::rename(&self.path, self.path.with_extension("jsonl.1"))
                .and_then(|()| {
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&self.path)
                })
                .map_err(|e| {
                    eprintln!(
                        "warning: access log {} rotation failed; disk logging off: {e}",
                        self.path.display()
                    )
                })
                .ok();
        }
        if let Some(f) = &mut self.file {
            use std::io::Write;
            if let Err(e) = writeln!(f, "{entry}") {
                eprintln!(
                    "warning: access log {} write failed; disk logging off: {e}",
                    self.path.display()
                );
                self.file = None;
            }
        }
        if self.entries.len() >= RELAY_LOG_CAP {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }
}

/// `POST /api/relay-access` — iroh-relay's access check. text/plain
/// `true`/`false`, never the JSON envelope: the relay string-matches the body.
fn relay_access(ctx: &Ctx, req: &Request) -> Response {
    let id = req
        .headers()
        .get("x-iroh-nodeid")
        .or_else(|| req.headers().get("x-iroh-endpoint-id"))
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let allowed = relay_allow(&load_members().unwrap_or_default(), id.as_deref());
    ctx.relay
        .lock()
        .unwrap()
        .push(id.as_deref(), allowed, "relay");
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/plain")],
        if allowed { "true" } else { "false" },
    )
        .into_response()
}

/// `/api/admin/*` — Member List, access/transfer logs and the Node Address,
/// JSON like the rest. `None` when the request is not an admin route.
async fn relay_admin(ctx: &Ctx, req: &ApiRequest) -> Option<Response> {
    const MEMBERS: &str = "/api/admin/relay/members";
    let path = req.path.split('?').next().unwrap_or("");
    let list = |l: &[Member]| serde_json::json!({ "members": l });
    // Corrupt member list: surface it and refuse writes rather than clobbering.
    let loaded = |r: Result<Vec<Member>, String>| {
        r.map_err(|e| json(StatusCode::CONFLICT, &serde_json::json!({ "detail": e })))
    };
    match (req.method.as_str(), path) {
        ("GET", MEMBERS) => Some(match loaded(load_members()) {
            Ok(l) => json(StatusCode::OK, &list(&l)),
            Err(resp) => resp,
        }),
        ("GET", "/api/admin/relay/log") => {
            let log = ctx.relay.lock().unwrap();
            Some(json(
                StatusCode::OK,
                &serde_json::json!({ "entries": log.entries }),
            ))
        }
        ("GET", "/api/admin/transfers") => {
            let log = ctx.transfers.lock().unwrap();
            Some(json(
                StatusCode::OK,
                &serde_json::json!({ "entries": log.entries() }),
            ))
        }
        ("GET", "/api/admin/node-address") => Some(node_address(ctx).await),
        // Upsert: add with a role (default none), or change an existing
        // member's role by POSTing the same id again.
        ("POST", MEMBERS) => {
            let body = req.body.as_ref();
            let id = body
                .and_then(|b| b["endpoint_id"].as_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !valid_endpoint_id(&id) {
                return Some(json(
                    StatusCode::BAD_REQUEST,
                    &serde_json::json!({ "detail": "endpoint_id must be 64 hex chars" }),
                ));
            }
            let role = match body.map(|b| &b["role"]) {
                None | Some(serde_json::Value::Null) => None,
                Some(v) => match serde_json::from_value(v.clone()) {
                    Ok(r) => Some(r),
                    Err(_) => {
                        return Some(json(
                            StatusCode::BAD_REQUEST,
                            &serde_json::json!({ "detail": "role must be none|read|read-write" }),
                        ))
                    }
                },
            };
            let _guard = ctx.relay.lock().unwrap(); // serialize read-modify-write
            let mut members = match loaded(load_members()) {
                Ok(l) => l,
                Err(resp) => return Some(resp),
            };
            upsert_member(&mut members, id, role);
            Some(persist(members, &list))
        }
        ("DELETE", p) if p.starts_with("/api/admin/relay/members/") => {
            let id = &p["/api/admin/relay/members/".len()..];
            let _guard = ctx.relay.lock().unwrap();
            let mut members = match loaded(load_members()) {
                Ok(l) => l,
                Err(resp) => return Some(resp),
            };
            members.retain(|m| !m.id.eq_ignore_ascii_case(id));
            Some(persist(members, &list))
        }
        _ => None,
    }
}

/// Member-list upsert. An absent `role` preserves an existing member's role
/// — old idempotent add scripts must not silently strip query rights — and
/// defaults a new member to `none`.
fn upsert_member(members: &mut Vec<Member>, id: String, role: Option<Role>) {
    match members.iter_mut().find(|m| m.id.eq_ignore_ascii_case(&id)) {
        Some(m) => {
            if let Some(role) = role {
                m.role = role;
            }
        }
        None => members.push(Member {
            id,
            role: role.unwrap_or_default(),
        }),
    }
}

fn persist(members: Vec<Member>, list: &impl Fn(&[Member]) -> serde_json::Value) -> Response {
    match save_members(&members) {
        Ok(()) => json(StatusCode::OK, &list(&members)),
        Err(e) => json(
            StatusCode::INTERNAL_SERVER_ERROR,
            &serde_json::json!({ "detail": format!("persist member list: {e}") }),
        ),
    }
}

/// `GET /api/admin/node-address` — the copyable locator members dial
/// (endpoint id + relay URL; a locator, not a capability). 409 until the
/// node is bound to a configured custom relay — n0's default relay set has
/// no single URL to encode.
async fn node_address(ctx: &Ctx) -> Response {
    let Some(id) = ctx.share.endpoint_id().await else {
        return json(
            StatusCode::CONFLICT,
            &serde_json::json!({ "detail": "share node is not bound" }),
        );
    };
    let p2p_config::RelaySetting::Custom(relay) = p2p_config::relay_setting() else {
        return json(
            StatusCode::CONFLICT,
            &serde_json::json!({ "detail": "node-address needs a configured relay (p2p_relay_url)" }),
        );
    };
    let id: linxiv_p2p::EndpointId = match id.parse() {
        Ok(id) => id,
        Err(e) => {
            return json(
                StatusCode::INTERNAL_SERVER_ERROR,
                &serde_json::json!({ "detail": format!("endpoint id: {e}") }),
            )
        }
    };
    let addr = linxiv_p2p::NodeAddress::new(id, relay.url().clone());
    json(
        StatusCode::OK,
        &serde_json::json!({ "node_address": addr.to_string() }),
    )
}

async fn dispatch(State(ctx): State<Ctx>, req: Request) -> Response {
    // Static, secretless — the only route outside bearer auth.
    if req.method() == axum::http::Method::GET && req.uri().path() == "/admin" {
        return (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
            ADMIN_HTML,
        )
            .into_response();
    }
    if let Some(rejection) = check_auth(&ctx, &req) {
        return rejection;
    }
    // Headless-only aggregate, answered here rather than in the shared router.
    if req.method() == axum::http::Method::GET && req.uri().path() == "/api/status" {
        return status(&ctx).await;
    }
    // iroh-relay access check: text/plain true/false, not the JSON envelope.
    if req.method() == axum::http::Method::POST && req.uri().path() == "/api/relay-access" {
        return relay_access(&ctx, &req);
    }
    let method = req.method().as_str().to_string();
    let path = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_default();
    let bytes = axum::body::to_bytes(req.into_body(), MAX_BODY)
        .await
        .unwrap_or_default();
    let body = if bytes.is_empty() {
        None
    } else {
        serde_json::from_slice(&bytes).ok()
    };
    let api_req = ApiRequest { method, path, body };
    if let Some(resp) = relay_admin(&ctx, &api_req).await {
        return resp;
    }

    let result = if api_req
        .path
        .trim_start_matches('/')
        .starts_with("api/share")
    {
        let spawn_sync = || spawn_interval_sync(&ctx);
        share::dispatch(&ctx.state, &ctx.share, &spawn_sync, api_req).await
    } else {
        route(&ctx.state, api_req).await
    };
    match result {
        Ok(value) => json(StatusCode::OK, &value),
        Err(e) => {
            let status =
                StatusCode::from_u16(e.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            json(status, &serde_json::json!({ "detail": e.detail }))
        }
    }
}

/// Most recent `synced_at` across share listings. All values come from one
/// `to_rfc3339` (UTC, fixed offset), so lexicographic max is chronological.
fn latest_synced_at<'a>(
    entries: impl IntoIterator<Item = &'a serde_json::Value>,
) -> Option<&'a str> {
    entries
        .into_iter()
        .filter_map(|e| e["synced_at"].as_str())
        .max()
}

/// `GET /api/status` — one-call health/config aggregate for a headless node.
async fn status(ctx: &Ctx) -> Response {
    let endpoint_id = ctx.share.endpoint_id().await;
    let settings = linxiv_core::config::UserSettings::load().ok();
    let get = |k: &str| settings.as_ref().and_then(|s| s.get(k));
    let relay = match p2p_config::relay_setting() {
        p2p_config::RelaySetting::Default => "default".to_string(),
        p2p_config::RelaySetting::RequireCustomButMissing => "require-custom-missing".into(),
        // `CustomRelay` also carries the auth token, so report the URL setting
        // it was parsed from — never the relay struct itself.
        p2p_config::RelaySetting::Custom(_) => get("p2p_relay_url")
            .and_then(|v| v.as_str())
            .unwrap_or("default")
            .into(),
    };
    // Reuse the share listings; a failed listing degrades to null counts.
    let hosted = share::list_shared(&ctx.state, &ctx.share)
        .ok()
        .and_then(|v| v["shared_projects"].as_array().cloned());
    let received = share::list_received(&ctx.state, &ctx.share)
        .ok()
        .and_then(|v| v["received"].as_array().cloned());
    json(
        StatusCode::OK,
        &serde_json::json!({
            "node_bound": endpoint_id.is_some(),
            "endpoint_id": endpoint_id,
            "relay": relay,
            "hosted_shares": hosted.as_ref().map(Vec::len),
            "received_shares": received.as_ref().map(Vec::len),
            "last_synced_at": latest_synced_at(hosted.iter().flatten().chain(received.iter().flatten())),
            "full_text_worker_enabled": get("full_text_worker_enabled").and_then(|v| v.as_bool()).unwrap_or(false),
            "home_feed_url_set": get("home_feed_url").and_then(|v| v.as_str()).is_some_and(|u| !u.trim().is_empty()),
            "uptime_secs": ctx.started.elapsed().as_secs(),
            "version": env!("CARGO_PKG_VERSION"),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::latest_synced_at;
    use serde_json::json;

    // relay_allow / member-list parsing tests live with the code in
    // `linxiv_server::remote_query`.

    /// The admin page is a blind consumer of these routes; renaming one must
    /// break this test, not the page at runtime. sessionStorage is the token
    /// storage contract (never localStorage/URL).
    #[test]
    fn admin_html_matches_api_surface() {
        for needle in [
            "/api/status",
            "/api/admin/relay/members",
            "/api/admin/relay/log",
            "/api/admin/transfers",
            "/api/admin/node-address",
            "sessionStorage",
        ] {
            assert!(super::ADMIN_HTML.contains(needle), "missing {needle}");
        }
        assert!(!super::ADMIN_HTML.contains("localStorage"));
    }

    #[test]
    fn upsert_absent_role_preserves_existing_and_defaults_new_to_none() {
        use super::{upsert_member, Member, Role};
        let id = "ab".repeat(32);
        let mut members = Vec::new();
        // New member, no role: defaults to none.
        upsert_member(&mut members, id.clone(), None);
        assert_eq!(
            members,
            vec![Member {
                id: id.clone(),
                role: Role::None
            }]
        );
        // Role grant sticks (case-insensitive id match, no duplicate).
        upsert_member(&mut members, id.to_uppercase(), Some(Role::Read));
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].role, Role::Read);
        // Idempotent re-add without a role: rights are preserved, not reset.
        upsert_member(&mut members, id, None);
        assert_eq!(members[0].role, Role::Read);
    }

    /// Restart survival: a reopened log continues the sequence and still
    /// holds the persisted entries.
    #[test]
    fn relay_log_persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay_access_log.jsonl");
        let mut log = super::RelayLog::open(path.clone());
        log.push(Some("aa"), false, "relay");
        log.push(None, true, "api");
        drop(log);
        let mut reopened = super::RelayLog::open(path);
        assert_eq!(reopened.entries.len(), 2);
        assert_eq!(reopened.seq, 2);
        reopened.push(Some("bb"), true, "relay");
        assert_eq!(reopened.entries.back().unwrap()["seq"], 3);
    }

    /// A restart right after rotation still seeds the tail (and seq) from the
    /// rotated generation.
    #[test]
    fn relay_log_reads_rotated_generation_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay_access_log.jsonl");
        let mut log = super::RelayLog::open(path.clone());
        log.push(Some("aa"), false, "relay");
        log.push(None, true, "api");
        drop(log);
        std::fs::rename(&path, path.with_extension("jsonl.1")).unwrap();
        let reopened = super::RelayLog::open(path);
        assert_eq!(reopened.entries.len(), 2);
        assert_eq!(reopened.seq, 2);
    }

    /// A torn final line (crash mid-write) is newline-terminated on open so
    /// the next entry doesn't merge into it.
    #[test]
    fn relay_log_heals_torn_final_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay_access_log.jsonl");
        std::fs::write(&path, "{\"seq\":1,\"allowed\":true}\n{\"seq\":2,\"allo").unwrap();
        let mut log = super::RelayLog::open(path.clone());
        assert_eq!(log.seq, 1);
        log.push(None, true, "api");
        drop(log);
        let reopened = super::RelayLog::open(path);
        assert_eq!(reopened.entries.len(), 2);
        assert_eq!(reopened.seq, 2);
    }

    #[test]
    fn feed_poll_period_defaults_and_rejects_nonpositive() {
        use super::{feed_poll_period, FEED_POLL_DEFAULT};
        use std::time::Duration;
        assert_eq!(feed_poll_period(None), FEED_POLL_DEFAULT);
        assert_eq!(feed_poll_period(Some(0)), FEED_POLL_DEFAULT);
        assert_eq!(feed_poll_period(Some(-5)), FEED_POLL_DEFAULT);
        assert_eq!(feed_poll_period(Some(i64::MAX)), FEED_POLL_DEFAULT);
        assert_eq!(feed_poll_period(Some(5)), Duration::from_secs(300));
    }

    #[test]
    fn clean_log_id_strips_control_chars_and_clamps() {
        assert_eq!(super::clean_log_id("ab\x1b[31m\ncd\r\0"), "ab[31mcd");
        assert_eq!(super::clean_log_id(&"x".repeat(500)).len(), 128);
    }

    #[test]
    fn latest_synced_at_picks_max_and_skips_nulls() {
        let entries = [
            json!({ "synced_at": "2026-08-01T00:00:00+00:00" }),
            json!({ "synced_at": serde_json::Value::Null }), // pending mirror
            json!({ "synced_at": "2026-09-01T12:30:00+00:00" }),
        ];
        assert_eq!(
            latest_synced_at(&entries),
            Some("2026-09-01T12:30:00+00:00")
        );
        assert_eq!(latest_synced_at(&[]), None);
    }
}

fn json(status: StatusCode, value: &serde_json::Value) -> Response {
    (
        status,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        serde_json::to_vec(value).unwrap_or_default(),
    )
        .into_response()
}
