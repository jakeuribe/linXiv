//! `linxiv://` custom URI scheme — serves PDF bytes to the webview in-process,
//! replacing the HTTP `GET /api/papers/{id}/pdf` and `GET /api/pdf/proxy`
//! endpoints. The `invoke()`-based `api` command returns JSON and cannot stream
//! binary into react-pdf / an `<iframe src>`, so these two flows need a real URL.
//!
//! Webview URL form (Tauri docs): `linxiv://localhost/<path>` on Linux/macOS,
//! `http://linxiv.localhost/<path>` on Windows — both reach `req.uri().path()`.
//! Two routes:
//!   `/papers/<source_id>/pdf?version=N`  → the locally-saved PDF bytes (404 if not
//!                                           on disk); only used for saved papers.
//!   `/pdf-proxy?url=<remote>`            → SSRF-guarded arXiv proxy (host allowlist
//!                                           + per-redirect re-check, via core).

use std::borrow::Cow;

use tauri::http::{header, Request, Response, StatusCode};
use tauri::{AppHandle, Manager, Runtime, UriSchemeContext, UriSchemeResponder};

use linxiv_core::error::CoreError;
use linxiv_core::service::paper::{self as svc_paper, Paper};
use linxiv_core::sources::http as core_http;

use crate::route::{pct_decode, pdfs::resolve_local_pdf};
use crate::state::AppState;

/// The scheme name registered on the Tauri builder.
pub const SCHEME: &str = "linxiv";

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
        ["papers", source_id, "pdf"] => serve_local_pdf(app, source_id, query),
        ["pdf-proxy"] => serve_proxy(query).await,
        _ => empty(StatusCode::NOT_FOUND),
    }
}

/// `/papers/<source_id>/pdf?version=N` — the saved PDF on disk. Mirrors
/// `_resolve_local_pdf`; 404 if there is no local file (the consumers only call
/// this for `has_pdf` papers, so the remote-redirect branch isn't needed here).
fn serve_local_pdf<R: Runtime>(
    app: &AppHandle<R>,
    source_id: &str,
    query: &str,
) -> Response<Cow<'static, [u8]>> {
    let version = query_get(query, "version").and_then(|v| v.parse::<i64>().ok());
    let state = app.state::<AppState>();
    let pdf_dir = state.pdf_dir.clone();
    // Resolve the path under the DB lock; read the file bytes outside it.
    let resolved = state.with_conn(|conn| {
        let paper = svc_paper::get(
            conn,
            &Paper { source_id: Some(source_id.to_string()), version, ..Default::default() },
        )
        .ok()
        .flatten()?;
        let ver = version.unwrap_or(paper.version);
        resolve_local_pdf(&pdf_dir, paper.pdf_path.as_deref(), &paper.source_id, ver)
    });
    match resolved.and_then(|p| std::fs::read(p).ok()) {
        Some(bytes) => pdf_response(bytes),
        None => empty(StatusCode::NOT_FOUND),
    }
}

/// `/pdf-proxy?url=<remote>` — `api_pdf_proxy`. Host-allowlisted + redirect-guarded
/// fetch through core; 400 host-not-allowed, 502 upstream.
async fn serve_proxy(query: &str) -> Response<Cow<'static, [u8]>> {
    let Some(url) = query_get(query, "url") else {
        return empty(StatusCode::BAD_REQUEST);
    };
    match core_http::get_guarded(&url, core_http::ARXIV_HOSTS).await {
        Ok(resp) if resp.status().is_success() => match resp.bytes().await {
            Ok(bytes) => pdf_response(bytes.to_vec()),
            Err(_) => empty(StatusCode::BAD_GATEWAY),
        },
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
