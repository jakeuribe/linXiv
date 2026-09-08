//! Dev-only HTTP shim (D32): serves `/api/*` by dispatching into the SAME
//! in-process router the Tauri app uses (Vite proxies `/api` → :8000). NOT shipped.

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::any,
    Router,
};

use linxiv_server::route::{route, ApiRequest};
use linxiv_server::state::AppState;

/// Base64 file uploads ride the JSON body, so allow a large request body.
const MAX_BODY: usize = 200 * 1024 * 1024;
const ADDR: &str = "127.0.0.1:8000";

#[tokio::main]
async fn main() {
    let state = Arc::new(AppState::new().expect("init app state"));
    let app = Router::new().fallback(any(dispatch)).with_state(state);
    let listener = tokio::net::TcpListener::bind(ADDR)
        .await
        .expect("bind dev shim");
    eprintln!("linxiv dev shim on http://{ADDR} — Vite proxies /api here.");
    axum::serve(listener, app).await.expect("dev shim serve");
}

async fn dispatch(State(state): State<Arc<AppState>>, req: Request) -> Response {
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

    match route(&state, ApiRequest { method, path, body }).await {
        Ok(value) => json(StatusCode::OK, &value),
        Err(e) => {
            let status =
                StatusCode::from_u16(e.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            json(status, &serde_json::json!({ "detail": e.detail }))
        }
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
