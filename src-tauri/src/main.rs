#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod integrations;
mod protocol;
mod route;
mod state;

use state::AppState;

use tauri::Manager;
use tauri_plugin_opener::OpenerExt;

/// Open a locally-stored PDF in the OS default viewer.
///
/// The path comes from our own backend's `pdf-path` route, but this command is
/// reachable over IPC, so we re-validate before handing the path to the OS: it
/// must be an absolute path to an existing `.pdf` file. We open from Rust rather
/// than the JS opener plugin on purpose — the plugin's `open_path` is scope-gated
/// against a static capability glob, and the PDF lives under a per-OS data
/// directory that's awkward to express as one. The Rust opener API is not
/// scope-gated, so this resolves the "view in system viewer fails" bug.
#[tauri::command]
fn open_pdf_in_system(app: tauri::AppHandle, path: String) -> Result<(), String> {
    let candidate = std::path::Path::new(&path);
    if !candidate.is_absolute() {
        return Err("Refusing to open a non-absolute path".to_string());
    }
    if !candidate.is_file() {
        return Err("PDF file not found on disk".to_string());
    }
    let is_pdf = candidate
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false);
    if !is_pdf {
        return Err("Refusing to open a non-PDF file in the system viewer".to_string());
    }
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|e| format!("System viewer could not open the PDF: {e}"))
}

fn main() {
    tauri::Builder::default()
        // linxiv:// serves PDF bytes to the webview and bridges the graph iframe's
        // /api/* GETs — the in-process replacement for what invoke() can't stream.
        .register_asynchronous_uri_scheme_protocol(protocol::SCHEME, |ctx, req, responder| {
            protocol::handler(ctx, req, responder)
        })
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_texbrain::init())
        .setup(|app| {
            // In-process backend: open the DB once and manage it. The webview
            // reaches linxiv-core through the `api` invoke command + the linxiv://
            // scheme — no Python sidecar, no HTTP hop, nothing to spawn or reap.
            app.manage(AppState::new().map_err(|e| e.to_string())?);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            route::api,
            open_pdf_in_system,
            integrations::is_cli_installed,
            integrations::install_cli,
            integrations::uninstall_cli,
            integrations::list_mcp_clients,
            integrations::install_mcp,
            integrations::uninstall_mcp,
            integrations::is_mcp_installed,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
