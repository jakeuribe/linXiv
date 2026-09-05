//! The Tauri layer over `linxiv-server`: the `api`/`share_api` invoke commands
//! and the AppHandle-resolving spawners for the background tasks. Everything
//! here is a thin wrapper — the logic lives in the server crate so the
//! headless/dev-server bins build without Tauri.

use serde_json::Value;
use tauri::Manager;

use linxiv_server::route::share::{self, ShareState};
use linxiv_server::route::{route, ApiError, ApiRequest};
use linxiv_server::state::AppState;
use linxiv_server::{full_text_worker, share_sync};

/// The Tauri command the webview invokes. Thin wrapper over `route`.
#[tauri::command]
pub async fn api(state: tauri::State<'_, AppState>, req: ApiRequest) -> Result<Value, ApiError> {
    route(state.inner(), req).await
}

/// The Tauri command the webview invokes for `/api/share/*`. Mirrors `api`'s
/// `{method, path, body}` shape but resolves `ShareState` alongside `AppState`.
#[tauri::command]
pub async fn share_api(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    share: tauri::State<'_, ShareState>,
    req: ApiRequest,
) -> Result<Value, ApiError> {
    let spawn_sync = move || spawn_interval_sync(app.clone());
    share::dispatch(state.inner(), share.inner(), &spawn_sync, req).await
}

/// Best-effort sync loop over every share, sequential, log-and-continue.
/// The task dies with the process. (The headless bin spawns its own twin.)
pub fn spawn_interval_sync(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            let state = app.state::<AppState>();
            let share = app.state::<ShareState>();
            share_sync::sync_all(&state, &share).await;
            share_sync::next_sync_due().await;
        }
    });
}

/// Spawn the full-text worker for the life of the app; restart-on-panic
/// semantics live in `full_text_worker::supervise`.
pub fn spawn_full_text_worker(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(full_text_worker::supervise(move || {
        let app = app.clone();
        async move { full_text_worker::run(&app.state::<AppState>()).await }
    }));
}
