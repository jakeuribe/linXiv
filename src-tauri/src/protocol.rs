//! `linxiv://` custom URI scheme — serves PDF bytes to the webview in-process,
//! replacing the HTTP `GET /api/papers/{id}/pdf` and `GET /api/pdf/proxy`
//! endpoints. The `invoke()`-based `api` command returns JSON and cannot stream
//! binary into react-pdf / an `<iframe src>`, so these two flows need a real URL.
//!
//! Webview URL form (Tauri docs): `linxiv://localhost/<path>` on Linux/macOS,
//! `http://linxiv.localhost/<path>` on Windows — both reach `req.uri().path()`.
//! Two routes (ids/urls travel as query params, never path segments, so an
//! old-style `math-ph/0309136` id can't be mangled by URI slash normalization):
//!   `/pdf?id=<source_id>&version=N` → the locally-saved PDF bytes (404 if not on
//!                                     disk); only used for saved papers.
//!   `/pdf-proxy?url=<remote>`       → SSRF-guarded arXiv proxy (host allowlist +
//!                                     per-redirect re-check, via core).

use std::borrow::Cow;
use std::time::Duration;

use tauri::http::{header, Request, Response, StatusCode};
use tauri::{AppHandle, Manager, Runtime, UriSchemeContext, UriSchemeResponder};

use serde_json::json;

use linxiv_core::error::CoreError;
use linxiv_core::service::paper::{self as svc_paper, Paper};
use linxiv_core::sources::http as core_http;

use crate::route::{pct_decode, pdfs::resolve_local_pdf, route, ApiRequest};
use crate::state::AppState;

/// The scheme name registered on the Tauri builder.
pub const SCHEME: &str = "linxiv";

/// Total ceiling on a proxied fetch (connect + transfer), like Python's
/// `_PDF_PROXY_TIMEOUT`. The shared core client only has connect/per-read timeouts.
const PROXY_TIMEOUT: Duration = Duration::from_secs(30);
/// Cap the buffered proxy body (the URI-scheme responder takes a complete
/// `Response`, so we can't stream — bound the memory instead). Matches the
/// upload limit `_MAX_PDF_BYTES`.
const MAX_PDF_BYTES: u64 = 100 * 1024 * 1024;

/// Async protocol handler: hand the work to the runtime so the proxy route can
/// `.await` the network without blocking the webview thread.
pub fn handler<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    req: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = ctx.app_handle().clone();
    tauri::async_runtime::spawn(async move {
        responder.respond(serve(&app, req).await);
    });
}

async fn serve<R: Runtime>(app: &AppHandle<R>, req: Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let segs: Vec<String> = req
        .uri()
        .path()
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .map(pct_decode)
        .collect();
    let query = req.uri().query().unwrap_or("");
    match segs.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["pdf"] => serve_local_pdf(app, query),
        ["pdf-proxy"] => serve_proxy(query).await,
        // The graph iframe is plain HTML/JS (no `invoke`), so it fetches its data
        // over this scheme: bridge `/api/*` GETs into the in-process router as JSON.
        ["api", ..] => serve_api_json(app, &req).await,
        _ => empty(StatusCode::NOT_FOUND),
    }
}

/// Run an `/api/*` request through the JSON router and return its result as an
/// `application/json` response (for the graph iframe's `fetch`).
async fn serve_api_json<R: Runtime>(
    app: &AppHandle<R>,
    req: &Request<Vec<u8>>,
) -> Response<Cow<'static, [u8]>> {
    // Read-only bridge: the graph iframe only GETs. Mutations go through the
    // `invoke('api')` transport, not this scheme.
    if req.method() != tauri::http::Method::GET {
        return empty(StatusCode::METHOD_NOT_ALLOWED);
    }
    let path = match req.uri().query() {
        Some(q) => format!("{}?{}", req.uri().path(), q),
        None => req.uri().path().to_string(),
    };
    let api_req = ApiRequest { method: req.method().as_str().to_string(), path, body: None };
    let state = app.state::<AppState>();
    match route(&state, api_req).await {
        Ok(value) => json_response(StatusCode::OK, &value),
        Err(e) => {
            let status = StatusCode::from_u16(e.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            json_response(status, &json!({ "detail": e.detail }))
        }
    }
}

fn json_response(status: StatusCode, value: &serde_json::Value) -> Response<Cow<'static, [u8]>> {
    let body = serde_json::to_vec(value).unwrap_or_default();
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Cow::Owned(body))
        .expect("static response builder")
}

/// `/pdf?id=<source_id>&version=N` — the saved PDF on disk. Mirrors
/// `_resolve_local_pdf`; 404 if there is no local file (the consumers only call
/// this for `has_pdf` papers, so the remote-redirect branch isn't needed here).
fn serve_local_pdf<R: Runtime>(app: &AppHandle<R>, query: &str) -> Response<Cow<'static, [u8]>> {
    let Some(source_id) = query_get(query, "id") else {
        return empty(StatusCode::BAD_REQUEST);
    };
    // Query(default=None, ge=1): a present-but-invalid version is a client error,
    // not a silent fall-through to the latest version.
    let version = match query_get(query, "version") {
        None => None,
        Some(v) => match v.parse::<i64>() {
            Ok(n) if n >= 1 => Some(n),
            _ => return empty(StatusCode::BAD_REQUEST),
        },
    };
    let state = app.state::<AppState>();
    let pdf_dir = state.pdf_dir.clone();
    // Pull just the fields out under the DB lock; do the fs stats + read OUTSIDE it
    // so a slow stat can't widen the connection's critical section.
    let found = state.with_conn(|conn| {
        svc_paper::get(
            conn,
            &Paper { source_id: Some(source_id.clone()), version, ..Default::default() },
        )
        .ok()
        .flatten()
        .map(|p| (p.source_id, p.pdf_path, version.unwrap_or(p.version)))
    });
    let Some((sid, pdf_path, ver)) = found else {
        return empty(StatusCode::NOT_FOUND);
    };
    match resolve_local_pdf(&pdf_dir, pdf_path.as_deref(), &sid, ver).and_then(|p| std::fs::read(p).ok())
    {
        Some(bytes) => pdf_response(bytes),
        None => empty(StatusCode::NOT_FOUND),
    }
}

/// `/pdf-proxy?url=<remote>` — `api_pdf_proxy`. Host-allowlisted + redirect-guarded
/// fetch through core, under a total timeout; 400 host-not-allowed, 502 upstream,
/// 504 timeout, 413 oversized.
async fn serve_proxy(query: &str) -> Response<Cow<'static, [u8]>> {
    let Some(url) = query_get(query, "url") else {
        return empty(StatusCode::BAD_REQUEST);
    };
    match tokio::time::timeout(PROXY_TIMEOUT, fetch_proxy(&url)).await {
        Ok(resp) => resp,
        Err(_) => empty(StatusCode::GATEWAY_TIMEOUT),
    }
}

async fn fetch_proxy(url: &str) -> Response<Cow<'static, [u8]>> {
    match core_http::get_guarded(url, core_http::ARXIV_HOSTS).await {
        Ok(resp) if resp.status().is_success() => {
            if resp.content_length().is_some_and(|n| n > MAX_PDF_BYTES) {
                return empty(StatusCode::PAYLOAD_TOO_LARGE);
            }
            match resp.bytes().await {
                Ok(bytes) if bytes.len() as u64 <= MAX_PDF_BYTES => pdf_response(bytes.to_vec()),
                Ok(_) => empty(StatusCode::PAYLOAD_TOO_LARGE),
                Err(_) => empty(StatusCode::BAD_GATEWAY),
            }
        }
        Ok(_) => empty(StatusCode::BAD_GATEWAY), // raise_for_status() equivalent
        Err(CoreError::BadRequest(_)) => empty(StatusCode::BAD_REQUEST), // host not allowed
        Err(_) => empty(StatusCode::BAD_GATEWAY),
    }
}

fn pdf_response(bytes: Vec<u8>) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/pdf")
        .header(header::CONTENT_DISPOSITION, "inline")
        .body(Cow::Owned(bytes))
        .expect("static response builder")
}

fn empty(status: StatusCode) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(status)
        .body(Cow::Borrowed(&b""[..]))
        .expect("static response builder")
}

/// First value of `key` in a `k=v&k2=v2` query, percent-decoded. The webview
/// `encodeURIComponent`s the value, so its own `&`/`=` arrive as `%26`/`%3D`.
fn query_get(query: &str, key: &str) -> Option<String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| pct_decode(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_get_decodes_and_picks_the_named_key() {
        assert_eq!(query_get("version=3", "version").as_deref(), Some("3"));
        // an encoded arXiv url survives intact (its own ?&= are %-escaped)
        let q = "url=https%3A%2F%2Farxiv.org%2Fpdf%2F2204.12985";
        assert_eq!(query_get(q, "url").as_deref(), Some("https://arxiv.org/pdf/2204.12985"));
        assert_eq!(query_get("a=1&b=2", "b").as_deref(), Some("2"));
        assert_eq!(query_get("a=1", "missing"), None);
    }
}
