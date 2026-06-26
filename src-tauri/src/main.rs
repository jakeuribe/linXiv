#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod integrations;
mod protocol;
mod route;
mod state;

use state::AppState;

use std::net::TcpListener;
#[cfg(unix)]
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, RunEvent};
use tauri_plugin_opener::OpenerExt;

static API_PORT: OnceLock<u16> = OnceLock::new();
static API_READY: OnceLock<bool> = OnceLock::new();

const EXPECTED_SERVICE: &str = "linxiv-api";

/// Handle to the spawned API process, kept in managed state so `reap_api` can
/// signal it on exit (dropping the handle does not kill the process).
enum ApiChild {
    Sidecar(tauri_plugin_shell::process::CommandChild),
    Dev(std::process::Child),
}

impl ApiChild {
    fn pid(&self) -> u32 {
        match self {
            ApiChild::Sidecar(c) => c.pid(),
            ApiChild::Dev(c) => c.id(),
        }
    }
}

struct ApiProcessState(Mutex<Option<ApiChild>>);

/// Per-launch nonce passed to the API via `LINXIV_HEALTH_TOKEN` and required back
/// from `/api/health`, so a stale/foreign process on the port can't be adopted.
fn make_health_token() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}-{:x}", std::process::id(), nanos)
}

/// Bind 127.0.0.1:preferred, falling back to an OS-assigned ephemeral port.
/// Dropping the listener to hand the port to the API leaves a small race;
/// `wait_for_api`'s token check is what closes it.
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

/// Poll /api/health until it answers with our service name and echoes our token,
/// or false on timeout. The token rejects a stale build or a port-squatter.
fn wait_for_api(port: u16, max_attempts: u32, token: &str) -> bool {
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
                    let service = body.get("service").and_then(|v| v.as_str());
                    let echoed = body.get("token").and_then(|v| v.as_str());
                    if service == Some(EXPECTED_SERVICE) && echoed == Some(token) {
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

/// Startup cleanup of `linxiv-api` sidecars orphaned by a previous launcher that
/// exited without reaping. Reaps by ownership (see `owned_by_live_launcher`), not
/// `PPID == 1`. Unix/`/proc`-only; release-only. See ADR 0018.
#[cfg(unix)]
fn sweep_orphaned_sidecars() {
    let Some(launcher_name) = current_exe_name() else {
        return; // can't identify our launcher — reap nothing
    };
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Ok(pid) = name.parse::<i32>() else {
            continue; // non-numeric /proc entry (cpuinfo, self, …)
        };
        if pid <= 1 {
            continue;
        }
        if proc_argv0_basename(pid).as_deref() != Some(EXPECTED_SERVICE) {
            continue;
        }
        if owned_by_live_launcher(pid, &launcher_name) {
            continue;
        }
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
        eprintln!("[linxiv] reaped orphaned {EXPECTED_SERVICE} sidecar (pid {pid})");
    }
}

/// Whether `pid` is still owned by a live launcher: walk up through bootloader
/// links to a live launcher ancestor (owned) vs init/systemd/other (orphaned).
/// Bounded hop count guards against a cyclic chain. See ADR 0018.
#[cfg(unix)]
fn owned_by_live_launcher(pid: i32, launcher_name: &str) -> bool {
    let mut cur = pid;
    for _ in 0..8 {
        let Some(ppid) = proc_ppid(cur) else {
            return false;
        };
        if ppid <= 1 {
            return false; // re-parented to init — orphaned
        }
        // Match the launcher on its resolved exe, not argv[0].
        if proc_exe_basename(ppid).as_deref() == Some(launcher_name) {
            return true; // live launcher ancestor
        }
        if proc_argv0_basename(ppid).as_deref() == Some(EXPECTED_SERVICE) {
            cur = ppid; // bootloader — keep walking up
            continue;
        }
        return false; // systemd/init/other — orphaned
    }
    false
}

/// Exe-path basename with the kernel's `" (deleted)"` suffix stripped, so a
/// still-running process whose binary was replaced on disk (app self-update)
/// still matches by name.
#[cfg(unix)]
fn exe_basename(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    Some(name.strip_suffix(" (deleted)").unwrap_or(name).to_string())
}

/// Basename of /proc/<pid>'s resolved executable (None if unreadable).
#[cfg(unix)]
fn proc_exe_basename(pid: i32) -> Option<String> {
    exe_basename(&std::fs::read_link(format!("/proc/{pid}/exe")).ok()?)
}

/// Basename of /proc/<pid>'s argv[0] (None if gone or a kernel thread). Basename
/// match, not substring, so we don't misfire on processes that merely mention it.
#[cfg(unix)]
fn proc_argv0_basename(pid: i32) -> Option<String> {
    let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    // cmdline is NUL-separated argv; argv[0] is the program path.
    let argv0 = bytes.split(|b| *b == 0).next().unwrap_or(&[]);
    if argv0.is_empty() {
        return None;
    }
    let argv0 = String::from_utf8_lossy(argv0);
    Some(Path::new(argv0.as_ref()).file_name()?.to_str()?.to_string())
}

/// Basename of the currently-running launcher executable, e.g. `linxiv-app`.
#[cfg(unix)]
fn current_exe_name() -> Option<String> {
    exe_basename(&std::env::current_exe().ok()?)
}

/// Parent PID from /proc/<pid>/stat. Scans past the final ')' first, since the
/// `comm` field can contain spaces/parens; the layout after it is " state ppid …".
#[cfg(unix)]
fn proc_ppid(pid: i32) -> Option<i32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = &stat[stat.rfind(')')? + 1..];
    after_comm.split_whitespace().nth(1)?.parse().ok()
}

/// SIGTERM the bootloader's forked Python child directly (it holds the port), so
/// reaping doesn't depend on the bootloader forwarding the signal.
#[cfg(unix)]
fn signal_sidecar_children(parent_pid: i32) {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.parse::<i32>().ok()) else {
            continue;
        };
        if proc_ppid(pid) == Some(parent_pid)
            && proc_argv0_basename(pid).as_deref() == Some(EXPECTED_SERVICE)
        {
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
    }
}

/// Reap the spawned API on app exit. Release: re-validate the PID, then SIGTERM
/// the bootloader and its forked child. Dev: SIGTERM the whole process group.
/// See ADR 0018.
fn reap_api(handle: &AppHandle) {
    let Some(state) = handle.try_state::<ApiProcessState>() else {
        return;
    };
    let Ok(mut guard) = state.0.lock() else {
        return;
    };
    let Some(child) = guard.take() else {
        return;
    };
    let pid = child.pid() as i32;
    #[cfg(unix)]
    {
        match child {
            ApiChild::Sidecar(_c) => {
                if proc_argv0_basename(pid).as_deref() == Some(EXPECTED_SERVICE) {
                    signal_sidecar_children(pid);
                    unsafe {
                        libc::kill(pid, libc::SIGTERM);
                    }
                }
            }
            ApiChild::Dev(_c) => {
                // The dev child leads its own process group (see spawn), so the
                // negative PID signals uv + python + uvicorn together.
                unsafe {
                    libc::kill(-pid, libc::SIGTERM);
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        match child {
            ApiChild::Sidecar(c) => {
                let _ = c.kill();
            }
            ApiChild::Dev(mut c) => {
                let _ = c.kill();
            }
        }
    }
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
    let app = tauri::Builder::default()
        // linxiv:// serves PDF bytes to the webview in-process (replaces the HTTP
        // /api/papers/{id}/pdf + /api/pdf/proxy endpoints, which invoke() can't stream).
        .register_asynchronous_uri_scheme_protocol(
            protocol::SCHEME,
            |ctx, req, responder| protocol::handler(ctx, req, responder),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_texbrain::init())
        .manage(ApiProcessState(Mutex::new(None)))
        .setup(|app| {
            // Resolve OS app data dir — Python stores DB, PDFs, settings here
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let data_dir_str = data_dir.to_string_lossy().to_string();

            // In-process backend: open the DB once and manage it. The webview's
            // apiFetch routes through the `api` command to linxiv-core (no HTTP
            // hop). The Python sidecar below still spawns during Phase 5 for
            // coexistence (D31); Phase 6 deletes it and the port/health machinery.
            app.manage(AppState::new().map_err(|e| e.to_string())?);

            // Clean up any sidecars a previous launcher left orphaned (covers the
            // SIGKILL-on-rebuild case the on-exit reap can't see).
            #[cfg(unix)]
            sweep_orphaned_sidecars();

            let port = find_free_port(8000);
            API_PORT.set(port).expect("API_PORT already initialized");
            let port_str = port.to_string();

            let token = make_health_token();

            let spawned: Option<ApiChild>;

            // Dev: spawn via uv, as a process-group leader so the whole
            // uv → python → uvicorn tree is reapable on exit (ADR 0018).
            #[cfg(debug_assertions)]
            {
                #[cfg(unix)]
                use std::os::unix::process::CommandExt;

                let project_dir = std::env::current_dir().unwrap_or_default();
                let mut cmd = std::process::Command::new("uv");
                cmd.args(["run", "python", "-m", "api"])
                    .current_dir(&project_dir)
                    .env("CORS_ORIGINS", "tauri://localhost,http://tauri.localhost,http://localhost:5180")
                    .env("LINXIV_DATA_DIR", &data_dir_str)
                    .env("LINXIV_PORT", &port_str)
                    .env("LINXIV_HEALTH_TOKEN", &token);
                #[cfg(unix)]
                cmd.process_group(0);
                match cmd.spawn() {
                    Ok(child) => spawned = Some(ApiChild::Dev(child)),
                    Err(e) => {
                        eprintln!("[linxiv] failed to spawn dev API via uv: {e}");
                        spawned = None;
                    }
                }
            }

            // Release: spawn PyInstaller sidecar binary
            #[cfg(not(debug_assertions))]
            {
                use tauri_plugin_shell::ShellExt;
                match app.shell().sidecar("linxiv-api") {
                    Ok(cmd) => {
                        match cmd
                            .env("LINXIV_DATA_DIR", &data_dir_str)
                            .env("CORS_ORIGINS", "tauri://localhost,http://tauri.localhost")
                            .env("LINXIV_PORT", &port_str)
                            .env("LINXIV_HEALTH_TOKEN", &token)
                            .spawn()
                        {
                            Ok((_rx, child)) => spawned = Some(ApiChild::Sidecar(child)),
                            Err(e) => {
                                eprintln!("[linxiv] failed to spawn linxiv-api sidecar: {e}");
                                spawned = None;
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("[linxiv] failed to resolve linxiv-api sidecar: {e}");
                        spawned = None;
                    }
                }
            }

            if let Some(child) = spawned {
                let state = app.state::<ApiProcessState>();
                *state.0.lock().expect("ApiProcessState poisoned") = Some(child);
            }

            let api_ready = wait_for_api(port, 20, &token);
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
            route::api,
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
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // Reap the API child on every in-app exit path (ADR 0018).
    app.run(|handle, event| {
        if let RunEvent::ExitRequested { .. } | RunEvent::Exit = event {
            reap_api(handle);
        }
    });
}
