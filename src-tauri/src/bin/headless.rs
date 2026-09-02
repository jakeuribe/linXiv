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

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::any,
    Router,
};

use linxiv_app::route::share::ShareState;
use linxiv_app::route::{feed, route, share, ApiRequest};
use linxiv_app::state::AppState;
use linxiv_app::{full_text_worker, p2p_config, share_sync};

/// Base64 file uploads ride the JSON body, so allow a large request body.
const MAX_BODY: usize = 200 * 1024 * 1024;

#[derive(Clone)]
struct Ctx {
    state: Arc<AppState>,
    share: Arc<ShareState>,
    /// Bearer token every request must present; `None` only on loopback.
    token: Option<Arc<str>>,
    /// Process start, for `uptime_secs` in `GET /api/status`.
    started: Instant,
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
    };
    if node_bound && ctx.share.mark_sync_started() {
        spawn_interval_sync(&ctx);
    }
    // Idles until `full_text_worker_enabled` is switched on, same as the app.
    full_text_worker::spawn_headless(ctx.state.clone());
    spawn_feed_poll(ctx.state.clone());

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

/// Same 5-minute loop the app spawns, minus the `AppHandle`.
fn spawn_interval_sync(ctx: &Ctx) {
    let (state, share) = (ctx.state.clone(), ctx.share.clone());
    tokio::spawn(async move {
        loop {
            share_sync::sync_all(&state, &share).await;
            tokio::time::sleep(Duration::from_secs(300)).await;
        }
    });
}

/// The desktop app refreshes the home feed when the user opens the screen; an
/// always-on node polls instead. No-op while `home_feed_url` is unset — the
/// settings read each tick is a small JSON file.
/// `ponytail: fixed 30-minute cadence; make it a setting if tuning is wanted.`
const FEED_POLL: Duration = Duration::from_secs(30 * 60);

fn spawn_feed_poll(state: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            match linxiv_core::config::UserSettings::load() {
                Ok(s) => {
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
            tokio::time::sleep(FEED_POLL).await;
        }
    });
}

async fn dispatch(State(ctx): State<Ctx>, req: Request) -> Response {
    if let Some(rejection) = check_auth(&ctx, &req) {
        return rejection;
    }
    // Headless-only aggregate, answered here rather than in the shared router.
    if req.method() == axum::http::Method::GET && req.uri().path() == "/api/status" {
        return status(&ctx).await;
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
