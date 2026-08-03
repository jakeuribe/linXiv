#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use linxiv_app::route::share::ShareState;
use linxiv_app::state::AppState;
use linxiv_app::{integrations, protocol, route};

use linxiv_core::config;
use linxiv_share::ShareNode;

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
    if !candidate
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
    {
        return Err("Refusing to open a non-PDF file in the system viewer".to_string());
    }
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|e| format!("System viewer could not open the PDF: {e}"))
}

/// 32-byte DEK for the p2p key store (write-enforcement spec §8): the at-rest
/// encryption key for `device.key` / `auth.key` / keyhive `state.bin`.
///
/// Resolution order (spec §8): fetch-or-create in the OS keychain (service
/// "linXiv", account "p2p-dek") is primary; keychain unavailable
/// (headless/CI) → Argon2id-derive from `LINXIV_P2P_PASSPHRASE` if set; else
/// `None` with one logged warning, keeping today's plaintext key store. The
/// passphrase is inert whenever the keychain works.
fn p2p_dek() -> Option<[u8; 32]> {
    let unavailable = |e: &dyn std::fmt::Display| match passphrase_dek() {
        Some(dek) => Some(dek),
        None => {
            eprintln!("warning: OS keychain unavailable, p2p key store stays plaintext: {e}");
            None
        }
    };
    let entry = match keyring::Entry::new("linXiv", "p2p-dek") {
        Ok(entry) => entry,
        Err(e) => return unavailable(&e),
    };
    let parse = |hex: &str| -> Option<[u8; 32]> {
        let bytes: Option<Vec<u8>> = (hex.len() == 64 && hex.is_ascii())
            .then(|| {
                (0..64)
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
                    .collect()
            })
            .flatten();
        bytes.and_then(|b| <[u8; 32]>::try_from(b).ok())
    };
    let malformed = || {
        // Never clobber an entry we can't read — regenerating would orphan
        // any files sealed under the old DEK.
        eprintln!("warning: keychain p2p-dek entry is malformed, p2p key store stays plaintext");
        None
    };
    match entry.get_password() {
        Ok(hex) => match parse(&hex) {
            Some(dek) => Some(dek),
            None => malformed(),
        },
        Err(keyring::Error::NoEntry) => {
            let mut dek = [0u8; 32];
            if let Err(e) = getrandom::fill(&mut dek) {
                return unavailable(&e);
            }
            let hex: String = dek.iter().map(|b| format!("{b:02x}")).collect();
            if let Err(e) = entry.set_password(&hex) {
                // A DEK that isn't persisted must not encrypt anything; the
                // passphrase fallback is fine here — it's re-derivable.
                return unavailable(&e);
            }
            // First-run mint race: two instances launched together can both
            // see NoEntry and set different DEKs — the keychain keeps the
            // last write. Seal files only under the READ-BACK value, never
            // the local mint, or state.bin can end up sealed under a key the
            // keychain no longer holds (unrecoverable from the next launch).
            match entry.get_password() {
                Ok(stored) => match parse(&stored) {
                    Some(dek) => Some(dek),
                    None => malformed(),
                },
                Err(e) => unavailable(&e),
            }
        }
        Err(e) => unavailable(&e),
    }
}

/// Keychain-unavailable fallback (spec §8): derive the DEK from
/// `LINXIV_P2P_PASSPHRASE` via Argon2id, default params. Fixed app-level
/// salt, documented as such: the passphrase is per-deployment; the salt only
/// domain-separates this derivation (there is no per-install salt to store).
fn passphrase_dek() -> Option<[u8; 32]> {
    let pass = std::env::var("LINXIV_P2P_PASSPHRASE").ok()?;
    let mut dek = [0u8; 32];
    match argon2::Argon2::default().hash_password_into(
        pass.as_bytes(),
        b"linxiv-p2p-dek-v1",
        &mut dek,
    ) {
        Ok(()) => Some(dek),
        Err(e) => {
            eprintln!("warning: LINXIV_P2P_PASSPHRASE derivation failed, p2p key store stays plaintext: {e}");
            None
        }
    }
}

fn main() {
    tauri::Builder::default()
        // Prevent a second linXiv process from opening shared resources such as
        // the P2P blob database. Focus the existing window instead.
        .plugin(tauri_plugin_single_instance::init(
            |app, _args, _cwd| {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.unminimize();
                    let _ = window.show();
                    let _ = window.set_focus();
                }
             },
        ))
        // linxiv:// serves PDF bytes to the webview and bridges the graph iframe's
        // /api/* GETs — the in-process replacement for what invoke() can't stream.
        .register_asynchronous_uri_scheme_protocol(protocol::SCHEME, protocol::handler)
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_texbrain::init())
        .setup(|app| {
            // In-process backend: open the DB once and manage it. The webview
            // reaches linxiv-core through the `api` invoke command + the linxiv://
            // scheme — no Python sidecar, no HTTP hop, nothing to spawn or reap.
            app.manage(AppState::new().map_err(|e| e.to_string())?);
            // Background TeX full-text indexing, one paper at a time. Idles
            // unless `full_text_worker_enabled` is on (Settings → Library).
            linxiv_app::full_text_worker::spawn(app.handle().clone());
            // Quarantined CRDT "shared projects" store, managed beside AppState
            // (never a field of it). Reached only via the `share_api` command.
            // The iroh node binds async (the Endpoint bind is async); block on it
            // during setup so the network arms have a live node from first request.
            let share_dir = config::data_dir().join("share");
            std::fs::create_dir_all(&share_dir).map_err(|e| e.to_string())?;
            // Persisted device key lives beside (not inside) the served share dir.
            let p2p_dir = config::data_dir().join("p2p");
            let mut node_bound = false;
            // At-rest key-store encryption: resolve the DEK before the bind
            // (keychain access is sync; never call it from async context).
            let dek = p2p_dek();
            let share_state = match tauri::async_runtime::block_on(ShareNode::bind_with_dek(
                share_dir.clone(),
                &p2p_dir,
                dek,
            )) {
                Ok(node) => {
                    node_bound = true;
                    ShareState::with_node(share_dir, node)
                }
                Err(e) => {
                    eprintln!(
                        "warning: share node bind failed, sharing (plain and e2ee) and background sync disabled: {e}"
                    );
                    ShareState::new(share_dir)
                }
            };
            app.manage(share_state);
            if node_bound {
                // Background share sync: one pass now, then every 5 min.
                linxiv_app::share_sync::spawn_interval_sync(app.handle().clone());
            }
            // Point the pdfium loader at the libpdfium bundled under the app
            // resources (tauri.conf.json `bundle.resources` maps it into pdfium/).
            if std::env::var_os("LINXIV_PDFIUM_LIB").is_none() {
                let lib_name = if cfg!(target_os = "windows") {
                    "pdfium.dll"
                } else if cfg!(target_os = "macos") {
                    "libpdfium.dylib"
                } else {
                    "libpdfium.so"
                };
                if let Ok(dir) = app.path().resource_dir() {
                    let lib = dir.join("pdfium").join(lib_name);
                    let lib_bin = dir.join("pdfium").join("bin").join(lib_name);
                    if lib.is_file() {
                        std::env::set_var("LINXIV_PDFIUM_LIB", lib);
                    } else if lib_bin.is_file() {
                        std::env::set_var("LINXIV_PDFIUM_LIB", lib_bin);
                    } else if !cfg!(debug_assertions) {
                        eprintln!(
                            "linxiv: bundled pdfium lib not found at {} or {}; PDF metadata extraction will be degraded",
                            lib.display(),
                            lib_bin.display()
                        );
                    }
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            route::api,
            route::share::share_api,
            open_pdf_in_system,
            integrations::is_cli_installed,
            integrations::install_cli,
            integrations::uninstall_cli,
            integrations::list_mcp_clients,
            integrations::install_mcp,
            integrations::uninstall_mcp,
            integrations::is_mcp_installed,
            integrations::get_linux_package_kind,
            integrations::apply_linux_package_update,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // Explicit async teardown of the iroh endpoint + router on exit; Drop
            // alone can't run the async close.
            if let tauri::RunEvent::ExitRequested { .. } = event {
                if let Some(share) = app.try_state::<ShareState>() {
                    // Bounded teardown: Router::shutdown can wait on draining handlers,
                    // so cap it and let exit proceed if it overruns. The timeout future
                    // is built inside the async block so its timer registers within the
                    // runtime — constructing it outside block_on panics "no reactor".
                    let teardown = tauri::async_runtime::block_on(async {
                        tokio::time::timeout(std::time::Duration::from_secs(5), share.shutdown())
                            .await
                    });
                    match teardown {
                        Ok(Err(e)) => eprintln!("share node shutdown error: {e}"),
                        Err(_) => eprintln!("share node shutdown timed out; abandoning"),
                        Ok(Ok(())) => {}
                    }
                }
            }
        });
}
