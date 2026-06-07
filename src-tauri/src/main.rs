#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod integrations;

use std::net::TcpListener;
use std::sync::OnceLock;
use std::time::Duration;
use tauri::Manager;
use tauri_plugin_opener::OpenerExt;

static API_PORT: OnceLock<u16> = OnceLock::new();
static API_READY: OnceLock<bool> = OnceLock::new();

const EXPECTED_SERVICE: &str = "linxiv-api";

/// Try to bind 127.0.0.1:preferred. If that fails, bind 127.0.0.1:0 and let the
/// OS pick a free port. There is a small race between releasing the listener and
/// the API binding — wait_for_api guards against a rogue process by validating
/// the /api/health response body identifies itself as our service.
fn find_free_port(preferred: u16) -> u16 {
    if let Ok(listener) = TcpListener::bind(("127.0.0.1", preferred)) {
        drop(listener);
        return preferred;
    }
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .expect("failed to bind ephemeral port on 127.0.0.1");
    let port = listener
        .local_addr()
        .expect("failed to read ephemeral local_addr")
        .port();
    drop(listener);
    port
}

/// Poll /api/health until we get a 2xx response whose JSON body identifies the
/// service as our API. Returns false on timeout. Validating the service name
/// (not just the status code) prevents us from claiming success if a rogue
/// process grabbed the port between our bind-probe and uvicorn startup.
fn wait_for_api(port: u16, max_attempts: u32) -> bool {
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    let url = format!("http://127.0.0.1:{}/api/health", port);
    for i in 0..max_attempts {
        if let Ok(resp) = client.get(&url).send() {
            if resp.status().is_success() {
                if let Ok(body) = resp.json::<serde_json::Value>() {
                    if body.get("service").and_then(|v| v.as_str()) == Some(EXPECTED_SERVICE) {
                        return true;
                    }
                }
            }
        }
        if i < max_attempts - 1 {
            std::thread::sleep(Duration::from_millis(500 + (i as u64 * 200)));
        }
    }
    false
}

/// Open a locally-stored PDF in the OS default viewer.
///
/// The path comes from our own backend's `pdf-path` endpoint, but this command
/// is reachable over IPC, so we re-validate before handing the path to the OS:
/// it must be an absolute path to an existing `.pdf` file. We open from Rust
/// rather than the JS opener plugin on purpose — the plugin's `open_path` is
/// scope-gated against a static capability glob, and the PDF lives under a
/// per-OS data directory that's awkward to express as one. The Rust opener API
/// is not scope-gated, so this resolves the "view in system viewer fails" bug.
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

#[tauri::command]
fn get_api_port() -> Result<u16, String> {
    match (API_READY.get(), API_PORT.get()) {
        (Some(true), Some(port)) => Ok(*port),
        (Some(false), _) => Err("API failed to start within timeout".to_string()),
        _ => Err("API initialization incomplete".to_string()),
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        // Disk access for the embedded editor's "Open Folder" (ADR 0018):
        // scope is extended at runtime by the texbrain plugin's pick_folder
        // command (recursive — the dialog plugin's own pick grant is not),
        // so no static directory grants are needed in the capability file.
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        // Serves the downloaded Editor plugin (texbrain:// + texlive:// schemes)
        // and its install/check_updates/uninstall/status commands (ADR 0016).
        .plugin(tauri_plugin_texbrain::init())
        .setup(|app| {
            // Resolve OS app data dir — Python stores DB, PDFs, settings here
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let data_dir_str = data_dir.to_string_lossy().to_string();

            let port = find_free_port(8000);
            API_PORT.set(port).expect("API_PORT already initialized");
            let port_str = port.to_string();

            // Dev: spawn via uv (source). Dropping the Child does not kill the
            // process on Unix; we only capture the Result so we can log spawn
            // failures (uv not on PATH, etc.) instead of swallowing them.
            #[cfg(debug_assertions)]
            {
                let project_dir = std::env::current_dir().unwrap_or_default();
                if let Err(e) = std::process::Command::new("uv")
                    .args(["run", "python", "-m", "api"])
                    .current_dir(&project_dir)
                    .env("CORS_ORIGINS", "tauri://localhost,http://tauri.localhost,http://localhost:5180")
                    .env("LINXIV_DATA_DIR", &data_dir_str)
                    .env("LINXIV_PORT", &port_str)
                    .spawn()
                {
                    eprintln!("[linxiv] failed to spawn dev API via uv: {e}");
                }
            }

            // Release: spawn PyInstaller sidecar binary
            #[cfg(not(debug_assertions))]
            {
                use tauri_plugin_shell::ShellExt;
                match app.shell().sidecar("linxiv-api") {
                    Ok(cmd) => {
                        if let Err(e) = cmd
                            .env("LINXIV_DATA_DIR", &data_dir_str)
                            .env("CORS_ORIGINS", "tauri://localhost,http://tauri.localhost")
                            .env("LINXIV_PORT", &port_str)
                            .spawn()
                        {
                            eprintln!("[linxiv] failed to spawn linxiv-api sidecar: {e}");
                        }
                    }
                    Err(e) => eprintln!("[linxiv] failed to resolve linxiv-api sidecar: {e}"),
                }
            }

            let api_ready = wait_for_api(port, 20);
            API_READY.set(api_ready).expect("API_READY already initialized");

            let window = app.get_webview_window("main").expect("main window not found");
            if !api_ready {
                let _ = window.set_title("linXiv — API failed to start");
                eprintln!("[linxiv] API did not become healthy after retries on port {port}");
                // The JS bootstrap calls `get_api_port`, which now returns Err
                // and renders a startup-error screen — no silent fallback to 8000.
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_api_port,
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

#[cfg(test)]
mod tests {
    /// Cross-repo sync check for the bridge-protocol list (closes the
    /// "host-side test TODO" in tauri-plugin-texbrain's install.rs): parse
    /// SUPPORTED_BRIDGE_PROTOCOLS out of src/api/editorPlugin.ts — the
    /// hand-maintained guest-js copy that drives EditorPage's runtime
    /// protocol-mismatch warning — and compare it against the plugin's Rust
    /// authority. Same array-literal extraction as the plugin's
    /// `bridge_protocols_match_guest_js` test.
    #[test]
    fn bridge_protocols_match_host_editor_plugin_ts() {
        let ts = include_str!("../../src/api/editorPlugin.ts");
        let decl = ts
            .find("const SUPPORTED_BRIDGE_PROTOCOLS")
            .map(|i| &ts[i..])
            .expect("src/api/editorPlugin.ts must declare SUPPORTED_BRIDGE_PROTOCOLS");
        let rhs = decl
            .split_once('=')
            .map(|(_, r)| r)
            .expect("declaration must assign a value");
        // The `[` of the `number[]` type annotation sits before the `=`, so
        // scan for the array literal's brackets only after it.
        let inner = rhs
            .split_once('[')
            .and_then(|(_, rest)| rest.split_once(']'))
            .map(|(inner, _)| inner)
            .expect("declaration must be an array literal");
        let ts_protocols: Vec<u32> = inner
            .split(',')
            .map(|n| n.trim())
            .filter(|n| !n.is_empty()) // trailing comma
            .map(|n| n.parse().expect("numeric protocol versions"))
            .collect();
        assert_eq!(ts_protocols, tauri_plugin_texbrain::SUPPORTED_BRIDGE_PROTOCOLS);
    }
}
