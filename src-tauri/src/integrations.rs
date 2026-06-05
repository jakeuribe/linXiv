use std::path::PathBuf;
use tauri::{AppHandle, Manager};

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Return the platform target triple (e.g. "x86_64-unknown-linux-gnu").
fn target_triple() -> Result<String, String> {
    tauri::utils::platform::target_triple().map_err(|e| e.to_string())
}

/// Resolve the path to a bundled sidecar binary.
///
/// Tauri places sidecars next to the main executable with the target triple
/// stripped (e.g. `/usr/bin/linxiv` in a deb/rpm install, `target/debug/linxiv`
/// in dev) — the same rule `shell().sidecar()` uses. Resolve there first, and
/// fall back to `src-tauri/binaries/<name>-<triple>` for dev runs from the
/// project root where the staged copy is the only one present.
///
/// On Windows the `.exe` extension is appended automatically.
fn sidecar_path(app: &AppHandle, name: &str) -> Result<PathBuf, String> {
    let _ = app; // kept for signature stability; resolution is exe-relative

    #[cfg(target_os = "windows")]
    let filename = format!("{}.exe", name);
    #[cfg(not(target_os = "windows"))]
    let filename = name.to_string();

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe_dir = exe
        .parent()
        .ok_or("Executable has no parent directory")?;
    let beside_exe = exe_dir.join(&filename);
    if beside_exe.exists() {
        eprintln!("[linxiv] sidecar '{}' resolved to {}", name, beside_exe.display());
        return Ok(beside_exe);
    }

    // Dev fallback: staged binaries keep their target-triple suffix.
    let triple = target_triple()?;
    #[cfg(target_os = "windows")]
    let staged_name = format!("{}-{}.exe", name, triple);
    #[cfg(not(target_os = "windows"))]
    let staged_name = format!("{}-{}", name, triple);

    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    for base in [cwd.join("src-tauri"), cwd.clone()] {
        let staged = base.join("binaries").join(&staged_name);
        if staged.exists() {
            eprintln!("[linxiv] sidecar '{}' resolved to staged {}", name, staged.display());
            return Ok(staged);
        }
    }

    Err(format!(
        "Sidecar binary '{}' not found: looked next to executable ({}) and in src-tauri/binaries/{}",
        name,
        beside_exe.display(),
        staged_name
    ))
}

// ─────────────────────────────────────────────────────────────────────────────
// CLI commands
// ─────────────────────────────────────────────────────────────────────────────

/// Path of the CLI shim that `install_cli` manages.
///
/// This is the single source of truth for where the CLI integration lives:
/// `is_cli_installed`, `install_cli`, and `uninstall_cli` all resolve through
/// here so the check can never drift from what install/uninstall touch.
///
/// - Linux/macOS: symlink `~/.local/bin/linxiv`
/// - Windows: shim `%LOCALAPPDATA%\Programs\linxiv\linxiv.bat`
fn cli_shim_path() -> Result<PathBuf, String> {
    #[cfg(not(target_os = "windows"))]
    {
        let home = dirs_home().ok_or("Could not determine home directory")?;
        Ok(home.join(".local").join("bin").join("linxiv"))
    }

    #[cfg(target_os = "windows")]
    {
        let local_app_data = std::env::var("LOCALAPPDATA")
            .map_err(|_| "LOCALAPPDATA not set".to_string())?;
        Ok(PathBuf::from(local_app_data)
            .join("Programs")
            .join("linxiv")
            .join("linxiv.bat"))
    }
}

/// Check whether the CLI shim installed by `install_cli` is present.
///
/// Deliberately does NOT consult PATH: a deb/rpm install ships
/// `/usr/bin/linxiv` regardless, which made a PATH lookup report "installed"
/// even when the shim was never created.
#[tauri::command]
pub fn is_cli_installed() -> bool {
    let shim = match cli_shim_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[linxiv] is_cli_installed: cannot resolve shim path: {e}");
            return false;
        }
    };

    // symlink_metadata also catches dangling symlinks on unix.
    if shim.symlink_metadata().is_err() {
        eprintln!("[linxiv] is_cli_installed: no shim at {}", shim.display());
        return false;
    }

    // A shim pointing at a binary that no longer exists counts as not installed.
    if !shim.exists() {
        eprintln!(
            "[linxiv] is_cli_installed: shim at {} is dangling (target missing)",
            shim.display()
        );
        return false;
    }

    eprintln!("[linxiv] is_cli_installed: shim present at {}", shim.display());
    true
}

/// Install the bundled `linxiv` CLI sidecar so it is accessible as `linxiv`
/// on the user's PATH.
///
/// - Linux/macOS: creates a symlink `~/.local/bin/linxiv` → binary path.
/// - Windows: creates a `.bat` shim in `%LOCALAPPDATA%\Programs\linxiv\` and
///   adds that directory to the user's PATH registry key.
#[tauri::command]
pub fn install_cli(app: AppHandle) -> Result<(), String> {
    eprintln!("[linxiv] install_cli: resolving bundled CLI binary…");
    let binary = sidecar_path(&app, "linxiv")?;
    let shim = cli_shim_path()?;
    eprintln!(
        "[linxiv] install_cli: linking {} -> {}",
        shim.display(),
        binary.display()
    );

    let shim_dir = shim.parent().ok_or("Shim path has no parent directory")?;
    std::fs::create_dir_all(shim_dir)
        .map_err(|e| format!("Failed to create {}: {e}", shim_dir.display()))?;

    // Remove stale shim/symlink first so we can re-link.
    if shim.symlink_metadata().is_ok() {
        eprintln!("[linxiv] install_cli: removing stale shim at {}", shim.display());
        std::fs::remove_file(&shim)
            .map_err(|e| format!("Failed to remove stale shim {}: {e}", shim.display()))?;
    }

    #[cfg(not(target_os = "windows"))]
    {
        std::os::unix::fs::symlink(&binary, &shim)
            .map_err(|e| format!("Failed to create symlink {}: {e}", shim.display()))?;
        eprintln!("[linxiv] install_cli: symlink created at {}", shim.display());
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        let binary_str = binary.to_string_lossy();
        let content = format!("@echo off\n\"{binary_str}\" %*\n");
        std::fs::write(&shim, content)
            .map_err(|e| format!("Failed to write shim {}: {e}", shim.display()))?;
        eprintln!("[linxiv] install_cli: shim written to {}", shim.display());

        let dir_str = shim_dir.to_string_lossy();
        windows_path_add(dir_str.as_ref())?;
        eprintln!("[linxiv] install_cli: ensured {} is on user PATH", dir_str);
        Ok(())
    }
}

/// Remove the `linxiv` CLI shim/symlink installed by `install_cli`.
#[tauri::command]
pub fn uninstall_cli() -> Result<(), String> {
    let shim = cli_shim_path()?;

    if shim.symlink_metadata().is_ok() {
        eprintln!("[linxiv] uninstall_cli: removing shim at {}", shim.display());
        std::fs::remove_file(&shim)
            .map_err(|e| format!("Failed to remove shim {}: {e}", shim.display()))?;
    } else {
        eprintln!(
            "[linxiv] uninstall_cli: no shim at {} — nothing to remove",
            shim.display()
        );
    }

    #[cfg(target_os = "windows")]
    {
        let dir = shim.parent().ok_or("Shim path has no parent directory")?;
        let dir_str = dir.to_string_lossy();
        windows_path_remove(dir_str.as_ref())?;
        eprintln!("[linxiv] uninstall_cli: removed {} from user PATH", dir_str);
    }

    eprintln!("[linxiv] uninstall_cli: done");
    Ok(())
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
fn read_mcp_config(path: &PathBuf) -> Result<serde_json::Value, String> {
    if path.exists() {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        serde_json::from_str(&text).map_err(|e| e.to_string())
    } else {
        Ok(serde_json::json!({}))
    }
}

/// Write a JSON value back to disk (pretty-printed).
fn write_mcp_config(path: &PathBuf, value: &serde_json::Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

/// Return `true` when a client application appears to be present on this machine.
///
/// Most clients are detected by checking whether their config directory exists.
/// Claude Code is detected by looking for the `claude` binary on PATH, because
/// its config file lives directly in `~` (which always exists).
fn is_client_available(client_id: &str) -> bool {
    if client_id == "claude-code" {
        #[cfg(target_os = "windows")]
        let result = std::process::Command::new("where").arg("claude").output();
        #[cfg(not(target_os = "windows"))]
        let result = std::process::Command::new("which").arg("claude").output();
        return matches!(result, Ok(o) if o.status.success());
    }
    mcp_config_dir(client_id)
        .map(|d| d.exists())
        .unwrap_or(false)
}

/// Return `true` when a config file contains `mcpServers.linxiv`.
fn config_has_linxiv(path: &PathBuf) -> bool {
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
                .map(config_has_linxiv)
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
    let binary = sidecar_path(&app, "linxiv-mcp")?;
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
