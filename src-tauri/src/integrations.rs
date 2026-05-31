use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Return the platform target triple (e.g. "x86_64-unknown-linux-gnu").
///
/// Only needed in dev mode, where staged sidecars keep the triple suffix; in
/// release the suffix is stripped at bundle time.
#[cfg(debug_assertions)]
fn target_triple() -> Result<String, String> {
    tauri::utils::platform::target_triple().map_err(|e| e.to_string())
}

/// Resolve the on-disk path to a bundled sidecar binary for installation.
///
/// This is an *install-time* resolver (only called by `install_cli` /
/// `install_mcp`): on the AppImage branch it performs filesystem mutation
/// (creates a dir, copies the binary, sets the exec bit) — see below. The name
/// reflects that side effect; do not use it as a pure path lookup.
///
/// Returns a `PathBuf` (not a `tauri_plugin_shell` `Command`) on purpose:
/// callers symlink the path (`install_cli`) or serialize it into a client's
/// JSON config (`install_mcp`), neither of which the shell plugin's `Command`
/// API can provide.
///
/// In dev mode the binary lives under `<src-tauri>/binaries/` with the
/// target-triple-suffixed name (e.g. `linxiv-x86_64-unknown-linux-gnu`), as
/// produced by `scripts/stage_sidecar.py`. The directory is resolved relative
/// to `CARGO_MANIFEST_DIR` (the `src-tauri` crate dir) rather than the current
/// working directory, so resolution does not depend on where the app is run
/// from.
///
/// In release mode Tauri bundles `externalBin` sidecars *next to the main
/// application executable* and strips the target-triple suffix at bundle time,
/// so the binary is `<current_exe dir>/<name>` (matching how
/// `tauri_plugin_shell`'s `relative_command_path` / `app.shell().sidecar()`
/// resolves sidecars). It is NOT under `<resource_dir>/binaries/`.
///
/// AppImage caveat: under an AppImage, `current_exe()` points inside the
/// ephemeral FUSE mount (`/tmp/.mount_xxxx/...`) which is gone once the app
/// quits. A symlink or config entry pointing there would dangle. So when the
/// `$APPIMAGE` env var is set (the AppImage runtime exports the host
/// `.AppImage` path there), we copy the in-mount sidecar to a stable,
/// user-writable directory under `app_data_dir()/bin` and return that copy.
/// The copy survives app quit, but it becomes stale after an AppImage update —
/// the copied binary is not refreshed automatically, so the symlink / MCP
/// config keeps pointing at the older copied version until the user re-runs
/// install, which re-copies the current binary.
///
/// On Windows the `.exe` extension is appended automatically.
///
/// The existence check lives here (rather than at each call site) so it is a
/// single source of truth covering both `install_cli` and `install_mcp`: a
/// missing sidecar surfaces an actionable error instead of silently creating a
/// dangling symlink or a broken MCP config entry.
fn resolve_install_sidecar(app: &AppHandle, name: &str) -> Result<PathBuf, String> {
    // `app` is only used in release mode (AppImage data dir); silence the
    // unused-variable warning in dev where resolution is purely path-based.
    #[cfg(debug_assertions)]
    let _ = app;

    #[cfg(debug_assertions)]
    let path = {
        // Dev binaries keep the triple suffix (see scripts/stage_sidecar.py).
        let triple = target_triple()?;
        #[cfg(not(target_os = "windows"))]
        let filename = format!("{}-{}", name, triple);
        #[cfg(target_os = "windows")]
        let filename = format!("{}-{}.exe", name, triple);
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(filename)
    };

    #[cfg(not(debug_assertions))]
    let path = {
        // Release sidecars sit next to the main executable with the triple
        // suffix stripped at bundle time.
        #[cfg(not(target_os = "windows"))]
        let filename = name.to_string();
        #[cfg(target_os = "windows")]
        let filename = format!("{}.exe", name);
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let exe_dir = exe
            .parent()
            .ok_or("Could not determine executable directory")?;
        let in_mount = exe_dir.join(filename);

        // Under AppImage the in-mount path is ephemeral; copy to a stable
        // per-user dir and hand back the durable copy instead.
        if std::env::var("APPIMAGE").is_ok() {
            appimage_stable_copy(app, name, &in_mount)?
        } else {
            in_mount
        }
    };

    if !path.exists() {
        #[cfg(debug_assertions)]
        let msg = format!(
            "Sidecar binary '{}' not found at {}. The sidecar may not be \
             staged (run `npm run build:sidecar`).",
            name,
            path.display()
        );
        #[cfg(not(debug_assertions))]
        let msg = format!(
            "Sidecar binary '{}' was not found at {}. This looks like a \
             packaging problem — please reinstall the app or file a bug report.",
            name,
            path.display()
        );
        return Err(msg);
    }

    Ok(path)
}

/// Copy an ephemeral AppImage-mount sidecar to a stable, user-writable location
/// and return the path to the copy.
///
/// The destination is `app_data_dir()/bin/<name>` (e.g.
/// `~/.local/share/com.linxiv.app/bin/linxiv` on Linux). The copy is made
/// executable and overwrites any prior copy, so re-installing always picks up
/// the freshly mounted binary.
///
/// AppImage is a Linux-only packaging format, so this path never executes on
/// macOS or Windows (it still compiles there as part of the release build).
#[cfg(not(debug_assertions))]
fn appimage_stable_copy(
    app: &AppHandle,
    name: &str,
    in_mount: &Path,
) -> Result<PathBuf, String> {
    if !in_mount.exists() {
        return Err(format!(
            "Sidecar binary '{}' was not found at {} inside the AppImage. \
             This looks like a packaging problem — please reinstall the app \
             or file a bug report.",
            name,
            in_mount.display()
        ));
    }

    let bin_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("bin");
    std::fs::create_dir_all(&bin_dir).map_err(|e| e.to_string())?;

    // Copy to a temp file, chmod it, then atomically rename onto the final
    // destination. `rename` is atomic on the same filesystem (tmp and dest both
    // live in bin_dir), so a reinstall never leaves a partial/zero-byte dest,
    // and it sidesteps ETXTBSY: if the old binary is still running, replacing
    // its directory entry leaves the running process on its original inode.
    let dest = bin_dir.join(name);
    let tmp = bin_dir.join(format!("{}.tmp", name));

    if let Err(e) = std::fs::copy(in_mount, &tmp).map_err(|e| e.to_string()) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    #[cfg(unix)]
    {
        // `std::fs::copy` already carries the source's permission bits on Unix,
        // so this OR of `0o111` is defensive belt-and-suspenders for the case
        // where the staged source somehow lacks the exec bit — it does not
        // compensate for any perm loss in `copy` (there is none).
        use std::os::unix::fs::PermissionsExt;
        let chmod = || -> Result<(), String> {
            let mut perms = std::fs::metadata(&tmp).map_err(|e| e.to_string())?.permissions();
            perms.set_mode(perms.mode() | 0o111);
            std::fs::set_permissions(&tmp, perms).map_err(|e| e.to_string())
        };
        if let Err(e) = chmod() {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    }

    if let Err(e) = std::fs::rename(&tmp, &dest).map_err(|e| e.to_string()) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    Ok(dest)
}

// ─────────────────────────────────────────────────────────────────────────────
// CLI commands
// ─────────────────────────────────────────────────────────────────────────────

/// Check whether the `linxiv` CLI is available on PATH.
#[tauri::command]
pub fn is_cli_installed() -> bool {
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("where")
        .arg("linxiv")
        .output();

    #[cfg(not(target_os = "windows"))]
    let result = std::process::Command::new("which")
        .arg("linxiv")
        .output();

    match result {
        Ok(output) => output.status.success(),
        Err(_) => false,
    }
}

/// Install the bundled `linxiv` CLI sidecar so it is accessible as `linxiv`
/// on the user's PATH.
///
/// - Linux/macOS: creates a symlink `~/.local/bin/linxiv` → binary path.
/// - Windows: creates a `.bat` shim in `%LOCALAPPDATA%\Programs\linxiv\` and
///   adds that directory to the user's PATH registry key.
#[tauri::command]
pub fn install_cli(app: AppHandle) -> Result<(), String> {
    let binary = resolve_install_sidecar(&app, "linxiv")?;

    #[cfg(not(target_os = "windows"))]
    {
        let home = dirs_home().ok_or("Could not determine home directory")?;
        let bin_dir = home.join(".local").join("bin");
        std::fs::create_dir_all(&bin_dir).map_err(|e| e.to_string())?;
        let link = bin_dir.join("linxiv");
        // Remove stale symlink/file first so we can re-link.
        if link.exists() || link.symlink_metadata().is_ok() {
            std::fs::remove_file(&link).map_err(|e| e.to_string())?;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&binary, &link).map_err(|e| e.to_string())?;
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        let local_app_data = std::env::var("LOCALAPPDATA")
            .map_err(|_| "LOCALAPPDATA not set".to_string())?;
        let dir = PathBuf::from(local_app_data)
            .join("Programs")
            .join("linxiv");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

        let bat = dir.join("linxiv.bat");
        let binary_str = binary.to_string_lossy();
        let content = format!("@echo off\n\"{binary_str}\" %*\n");
        std::fs::write(&bat, content).map_err(|e| e.to_string())?;

        windows_path_add(dir.to_string_lossy().as_ref())?;
        Ok(())
    }
}

/// Remove the `linxiv` CLI shim/symlink installed by `install_cli`.
#[tauri::command]
pub fn uninstall_cli() -> Result<(), String> {
    #[cfg(not(target_os = "windows"))]
    {
        let home = dirs_home().ok_or("Could not determine home directory")?;
        let link = home.join(".local").join("bin").join("linxiv");
        if link.exists() || link.symlink_metadata().is_ok() {
            std::fs::remove_file(&link).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        let local_app_data = std::env::var("LOCALAPPDATA")
            .map_err(|_| "LOCALAPPDATA not set".to_string())?;
        let dir = PathBuf::from(local_app_data)
            .join("Programs")
            .join("linxiv");
        let bat = dir.join("linxiv.bat");
        if bat.exists() {
            std::fs::remove_file(&bat).map_err(|e| e.to_string())?;
        }
        windows_path_remove(dir.to_string_lossy().as_ref())?;
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MCP types & helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Status of a supported MCP client on this machine.
// NOTE: The struct is intentionally named `MpcClientStatus` (not `Mcp…`) to
// match the identifier agreed with the frontend.
#[derive(serde::Serialize)]
pub struct MpcClientStatus {
    pub id: String,
    pub name: String,
    /// `true` when linxiv is already registered in the client's config file.
    pub installed: bool,
    /// `true` when the client application appears to be present on this machine.
    pub available: bool,
}

/// Return the path to the MCP config file for a given client.
fn mcp_config_path(client_id: &str) -> Result<PathBuf, String> {
    let home = dirs_home().ok_or("Could not determine home directory")?;

    match client_id {
        "claude" => {
            #[cfg(target_os = "linux")]
            return Ok(home.join(".config").join("Claude").join("claude_desktop_config.json"));

            #[cfg(target_os = "macos")]
            return Ok(home
                .join("Library")
                .join("Application Support")
                .join("Claude")
                .join("claude_desktop_config.json"));

            #[cfg(target_os = "windows")]
            {
                let appdata = std::env::var("APPDATA")
                    .map_err(|_| "APPDATA not set".to_string())?;
                return Ok(PathBuf::from(appdata)
                    .join("Claude")
                    .join("claude_desktop_config.json"));
            }

            #[cfg(not(any(
                target_os = "linux",
                target_os = "macos",
                target_os = "windows"
            )))]
            Err(format!("Unsupported OS for client '{}'", client_id))
        }
        "cursor" => {
            #[cfg(not(target_os = "windows"))]
            return Ok(home.join(".cursor").join("mcp.json"));

            #[cfg(target_os = "windows")]
            {
                let appdata = std::env::var("APPDATA")
                    .map_err(|_| "APPDATA not set".to_string())?;
                return Ok(PathBuf::from(appdata).join("Cursor").join("mcp.json"));
            }
        }
        "antigravity" => {
            #[cfg(not(target_os = "windows"))]
            return Ok(home
                .join(".codeium")
                .join("antigravity")
                .join("mcp_config.json"));

            #[cfg(target_os = "windows")]
            {
                let appdata = std::env::var("APPDATA")
                    .map_err(|_| "APPDATA not set".to_string())?;
                return Ok(PathBuf::from(appdata)
                    .join("Codeium")
                    .join("Antigravity")
                    .join("mcp_config.json"));
            }
        }
        "claude-code" => Ok(home.join(".claude.json")),
        _ => Err(format!("Unknown MCP client: {}", client_id)),
    }
}

/// Return the config *directory* for a client (used to test whether the app is
/// installed without needing to find the client binary).
fn mcp_config_dir(client_id: &str) -> Option<PathBuf> {
    mcp_config_path(client_id)
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
}

/// Read the MCP JSON config file (or return an empty object if it doesn't
/// exist), then return the parsed value.
fn read_mcp_config(path: &Path) -> Result<serde_json::Value, String> {
    if path.exists() {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        serde_json::from_str(&text).map_err(|e| e.to_string())
    } else {
        Ok(serde_json::json!({}))
    }
}

/// Write a JSON value back to disk (pretty-printed).
fn write_mcp_config(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

/// Return `true` when a client application appears to be present on this machine.
///
/// Most clients are detected by checking whether their config directory exists.
///
/// Claude Code is detected by checking for its config paths in the home dir
/// (`~/.claude.json` or the `~/.claude/` state dir), NOT by probing PATH for the
/// `claude` binary. The PATH probe was unreliable: a GUI launched from the
/// desktop (GNOME/systemd) inherits a minimal PATH (`/usr/bin:/bin:…`) that omits
/// `~/.local/bin` where `claude` is installed, so `which claude` failed and the
/// client was wrongly reported as unavailable even though it was installed and
/// linxiv was registered. The home-dir config paths are PATH-independent and so
/// behave identically whether the app is launched from a terminal or the desktop.
fn is_client_available(client_id: &str) -> bool {
    if client_id == "claude-code" {
        // Claude Code creates `~/.claude.json` (config) and `~/.claude/` (state)
        // on first run; either is sufficient evidence it is present. We do NOT
        // fall back to `which claude`: it cannot fix the desktop-launch bug
        // (minimal PATH fails the probe regardless) and would only ever matter in
        // the narrow terminal-launched-but-never-run window — which is exactly
        // when there is no config and nothing to manage, so "unavailable" is the
        // correct answer there anyway. Not worth a cfg-gated process spawn.
        let Some(home) = dirs_home() else {
            return false;
        };
        // `~/.claude.json` is a file → `exists()`; `~/.claude` is definitionally a
        // directory → `is_dir()` (also rejects a stray non-dir file of that name).
        return home.join(".claude.json").exists() || home.join(".claude").is_dir();
    }
    mcp_config_dir(client_id)
        .map(|d| d.exists())
        .unwrap_or(false)
}

/// Return `true` when a config file contains `mcpServers.linxiv`.
fn config_has_linxiv(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    match read_mcp_config(path) {
        Ok(v) => v
            .get("mcpServers")
            .and_then(|s| s.get("linxiv"))
            .is_some(),
        Err(_) => false,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MCP commands
// ─────────────────────────────────────────────────────────────────────────────

/// Return all supported MCP clients with their current install/available status.
#[tauri::command]
pub fn list_mcp_clients() -> Vec<MpcClientStatus> {
    let clients = [
        ("claude", "Claude Desktop"),
        ("claude-code", "Claude Code"),
        ("cursor", "Cursor"),
        ("antigravity", "Antigravity"),
    ];

    clients
        .iter()
        .map(|(id, name)| {
            let config_path = mcp_config_path(id).ok();
            let installed = config_path
                .as_ref()
                .map(|p| config_has_linxiv(p))
                .unwrap_or(false);
            let available = is_client_available(id);

            MpcClientStatus {
                id: id.to_string(),
                name: name.to_string(),
                installed,
                available,
            }
        })
        .collect()
}

/// Register linxiv's MCP server in a client's config file.
///
/// Existing `mcpServers` entries are preserved; only the `"linxiv"` key is
/// added or overwritten.
#[tauri::command]
pub fn install_mcp(app: AppHandle, client_id: String) -> Result<(), String> {
    let binary = resolve_install_sidecar(&app, "linxiv-mcp")?;
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;

    let config_path = mcp_config_path(&client_id)?;
    let mut config = read_mcp_config(&config_path)?;

    let servers = config
        .as_object_mut()
        .ok_or("Config root is not a JSON object")?
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));

    let servers_obj = servers
        .as_object_mut()
        .ok_or("mcpServers is not a JSON object")?;

    servers_obj.insert(
        "linxiv".to_string(),
        serde_json::json!({
            "command": binary.to_string_lossy(),
            "args": [],
            "env": {
                "LINXIV_DATA_DIR": data_dir.to_string_lossy()
            }
        }),
    );

    write_mcp_config(&config_path, &config)
}

/// Remove the `"linxiv"` entry from a client's `mcpServers` config.
/// Succeeds silently if the file or key does not exist.
#[tauri::command]
pub fn uninstall_mcp(client_id: String) -> Result<(), String> {
    let config_path = mcp_config_path(&client_id)?;

    if !config_path.exists() {
        return Ok(());
    }

    let mut config = read_mcp_config(&config_path)?;

    if let Some(servers) = config
        .as_object_mut()
        .and_then(|o| o.get_mut("mcpServers"))
        .and_then(|s| s.as_object_mut())
    {
        servers.remove("linxiv");
    }

    write_mcp_config(&config_path, &config)
}

/// Check whether the `"linxiv"` MCP entry exists in a client's config file.
#[tauri::command]
pub fn is_mcp_installed(client_id: String) -> bool {
    match mcp_config_path(&client_id) {
        Ok(p) => config_has_linxiv(&p),
        Err(_) => false,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Platform utilities
// ─────────────────────────────────────────────────────────────────────────────

/// Cross-platform home directory lookup without pulling in the `dirs` crate.
fn dirs_home() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var("USERPROFILE")
            .ok()
            .map(PathBuf::from)
            .or_else(|| {
                let drive = std::env::var("HOMEDRIVE").ok()?;
                let path = std::env::var("HOMEPATH").ok()?;
                Some(PathBuf::from(format!("{}{}", drive, path)))
            })
    }

    #[cfg(not(target_os = "windows"))]
    {
        std::env::var("HOME").ok().map(PathBuf::from)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Windows PATH registry helpers (compiled only on Windows)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn windows_path_add(dir: &str) -> Result<(), String> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let env = hkcu
        .open_subkey_with_flags("Environment", KEY_READ | KEY_WRITE)
        .map_err(|e| e.to_string())?;

    let current_path: String = env.get_value("Path").unwrap_or_default();

    // Only append if the directory is not already on PATH.
    let entries: Vec<&str> = current_path.split(';').collect();
    if entries.iter().any(|e| e.eq_ignore_ascii_case(dir)) {
        return Ok(());
    }

    let new_path = if current_path.is_empty() {
        dir.to_string()
    } else {
        format!("{};{}", current_path, dir)
    };

    // NOTE: Running applications will not see this change until they restart;
    // broadcasting WM_SETTINGCHANGE would notify them but requires a Win32 call.
    env.set_value("Path", &new_path).map_err(|e| e.to_string())
}

#[cfg(target_os = "windows")]
fn windows_path_remove(dir: &str) -> Result<(), String> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let env = hkcu
        .open_subkey_with_flags("Environment", KEY_READ | KEY_WRITE)
        .map_err(|e| e.to_string())?;

    let current_path: String = env.get_value("Path").unwrap_or_default();

    let new_path: Vec<&str> = current_path
        .split(';')
        .filter(|e| !e.eq_ignore_ascii_case(dir))
        .collect();

    env.set_value("Path", &new_path.join(";"))
        .map_err(|e| e.to_string())
}
