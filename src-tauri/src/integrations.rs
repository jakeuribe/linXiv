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
fn appimage_stable_copy(app: &AppHandle, name: &str, in_mount: &Path) -> Result<PathBuf, String> {
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
        // OR of `0o111` in case the staged source lacks the exec bit.
        use std::os::unix::fs::PermissionsExt;
        let chmod = || -> Result<(), String> {
            let mut perms = std::fs::metadata(&tmp)
                .map_err(|e| e.to_string())?
                .permissions();
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
        let local_app_data =
            std::env::var("LOCALAPPDATA").map_err(|_| "LOCALAPPDATA not set".to_string())?;
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

    eprintln!(
        "[linxiv] is_cli_installed: shim present at {}",
        shim.display()
    );
    true
}

/// Install the bundled `linxiv` CLI sidecar so it is accessible as `linxiv`
/// on the user's PATH.
///
/// - Linux/macOS: creates a symlink `~/.local/bin/linxiv` → binary path.
/// - Windows: creates a `.bat` shim in `%LOCALAPPDATA%\Programs\linxiv\` and
///   adds that directory to the user's PATH registry key.
///
/// Dev builds refuse unless LINXIV_DEV_INSTALL=1 is set.
#[tauri::command]
pub fn install_cli(app: AppHandle) -> Result<(), String> {
    dev_install_guard(std::env::var("LINXIV_DEV_INSTALL").ok().as_deref())?;
    eprintln!("[linxiv] install_cli: resolving bundled CLI binary…");
    let binary = resolve_install_sidecar(&app, "linxiv")?;
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
        eprintln!(
            "[linxiv] install_cli: removing stale shim at {}",
            shim.display()
        );
        std::fs::remove_file(&shim)
            .map_err(|e| format!("Failed to remove stale shim {}: {e}", shim.display()))?;
    }

    #[cfg(not(target_os = "windows"))]
    {
        std::os::unix::fs::symlink(&binary, &shim)
            .map_err(|e| format!("Failed to create symlink {}: {e}", shim.display()))?;
        eprintln!(
            "[linxiv] install_cli: symlink created at {}",
            shim.display()
        );
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
        eprintln!(
            "[linxiv] uninstall_cli: removing shim at {}",
            shim.display()
        );
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
    /// `true` when linxiv is registered in the client's current config file.
    pub installed: bool,
    /// `true` when the client application appears to be present on this machine.
    pub available: bool,
    /// `true` when the registered command no longer exists on disk (e.g. the
    /// deleted Python-era sidecar) and the entry needs a reinstall.
    pub stale: bool,
}

/// Supported clients: (id, display name). Paths and config shape live in
/// `mcp_config_path_in` / `servers_key` / `client_app_markers`.
const MCP_CLIENTS: &[(&str, &str)] = &[
    ("claude", "Claude Desktop"),
    ("claude-code", "Claude Code"),
    ("cursor", "Cursor"),
    ("antigravity", "Antigravity"),
    ("windsurf", "Windsurf"),
    ("vscode", "VS Code"),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Os {
    Linux,
    Mac,
    Windows,
}

impl Os {
    fn current() -> Os {
        if cfg!(target_os = "macos") {
            Os::Mac
        } else if cfg!(target_os = "windows") {
            Os::Windows
        } else {
            Os::Linux
        }
    }
}

/// Filesystem roots client paths derive from; injected so tests can exercise
/// every OS's path table on any host.
struct Roots {
    os: Os,
    home: PathBuf,
    /// `%APPDATA%` — `None` off Windows.
    appdata: Option<PathBuf>,
    /// `%LOCALAPPDATA%` — `None` off Windows.
    local_appdata: Option<PathBuf>,
}

impl Roots {
    fn current() -> Result<Roots, String> {
        Ok(Roots {
            os: Os::current(),
            home: dirs_home().ok_or("Could not determine home directory")?,
            appdata: std::env::var("APPDATA").ok().map(PathBuf::from),
            local_appdata: std::env::var("LOCALAPPDATA").ok().map(PathBuf::from),
        })
    }
}

/// Key under which a client lists MCP servers in its config file. VS Code's
/// `mcp.json` uses `servers` (with a `type` per entry); the rest use the
/// reference `mcpServers` shape.
fn servers_key(client_id: &str) -> &'static str {
    match client_id {
        "vscode" => "servers",
        _ => "mcpServers",
    }
}

/// Path of the client's user-level MCP config file.
///
/// Cursor and Windsurf document home-based paths on every OS; Antigravity 2.0
/// shares Gemini's `~/.gemini/config/mcp_config.json`.
fn mcp_config_path_in(client_id: &str, roots: &Roots) -> Result<PathBuf, String> {
    let home = &roots.home;
    let appdata = || -> Result<&PathBuf, String> {
        roots
            .appdata
            .as_ref()
            .ok_or_else(|| "APPDATA not set".to_string())
    };
    let path = match (client_id, roots.os) {
        ("claude", Os::Linux) => home
            .join(".config")
            .join("Claude")
            .join("claude_desktop_config.json"),
        ("claude", Os::Mac) => home
            .join("Library")
            .join("Application Support")
            .join("Claude")
            .join("claude_desktop_config.json"),
        ("claude", Os::Windows) => appdata()?.join("Claude").join("claude_desktop_config.json"),
        ("claude-code", _) => home.join(".claude.json"),
        ("cursor", _) => home.join(".cursor").join("mcp.json"),
        ("antigravity", _) => home.join(".gemini").join("config").join("mcp_config.json"),
        ("windsurf", _) => home
            .join(".codeium")
            .join("windsurf")
            .join("mcp_config.json"),
        ("vscode", Os::Linux) => home
            .join(".config")
            .join("Code")
            .join("User")
            .join("mcp.json"),
        ("vscode", Os::Mac) => home
            .join("Library")
            .join("Application Support")
            .join("Code")
            .join("User")
            .join("mcp.json"),
        ("vscode", Os::Windows) => appdata()?.join("Code").join("User").join("mcp.json"),
        _ => return Err(format!("Unknown MCP client: {}", client_id)),
    };
    Ok(path)
}

/// Runtime wrapper over `mcp_config_path_in` for the current machine.
fn mcp_config_path(client_id: &str) -> Result<PathBuf, String> {
    mcp_config_path_in(client_id, &Roots::current()?)
}

/// Return paths to check for Antigravity MCP config in order of preference.
/// Checks the new Gemini path first, then falls back to the legacy Codeium path.
fn antigravity_config_paths(roots: &Roots) -> Vec<PathBuf> {
    let home = &roots.home;
    let mut paths = vec![home.join(".gemini").join("config").join("mcp_config.json")];
    // Legacy Codeium-era Antigravity path.
    let legacy = match roots.os {
        Os::Linux | Os::Mac => home
            .join(".codeium")
            .join("antigravity")
            .join("mcp_config.json"),
        Os::Windows => {
            if let Some(ad) = &roots.appdata {
                ad.join("Codeium")
                    .join("Antigravity")
                    .join("mcp_config.json")
            } else {
                return paths;
            }
        }
    };
    paths.push(legacy);
    paths
}

/// Paths whose existence indicates the client *application* is installed.
fn client_app_markers(client_id: &str, roots: &Roots) -> Vec<PathBuf> {
    let home = &roots.home;
    let lad = roots.local_appdata.as_ref();
    match (client_id, roots.os) {
        ("claude", Os::Mac) => vec![
            PathBuf::from("/Applications/Claude.app"),
            home.join("Applications").join("Claude.app"),
        ],
        ("claude", Os::Windows) => lad
            .map(|l| vec![l.join("AnthropicClaude")])
            .unwrap_or_default(),
        ("claude", Os::Linux) => vec![home.join(".config").join("Claude")],
        ("claude-code", _) => vec![home.join(".claude.json"), home.join(".claude")],
        ("cursor", Os::Mac) => vec![
            PathBuf::from("/Applications/Cursor.app"),
            home.join("Applications").join("Cursor.app"),
        ],
        ("cursor", Os::Windows) => lad
            .map(|l| vec![l.join("Programs").join("cursor")])
            .unwrap_or_default(),
        // ponytail: fixed path list; an unintegrated AppImage is undetectable
        ("cursor", Os::Linux) => vec![
            PathBuf::from("/usr/bin/cursor"),
            PathBuf::from("/usr/local/bin/cursor"),
            PathBuf::from("/opt/Cursor"),
            PathBuf::from("/usr/share/cursor"),
            home.join(".local").join("bin").join("cursor"),
            home.join(".local")
                .join("share")
                .join("applications")
                .join("cursor.desktop"),
            PathBuf::from("/usr/share/applications/cursor.desktop"),
        ],
        ("antigravity", Os::Mac) => vec![
            PathBuf::from("/Applications/Antigravity.app"),
            home.join("Applications").join("Antigravity.app"),
        ],
        ("antigravity", Os::Windows) => lad
            .map(|l| vec![l.join("Programs").join("Antigravity")])
            .unwrap_or_default(),
        ("antigravity", Os::Linux) => vec![
            PathBuf::from("/usr/bin/antigravity"),
            PathBuf::from("/usr/share/antigravity"),
            PathBuf::from("/opt/antigravity"),
            PathBuf::from("/usr/share/applications/antigravity.desktop"),
            home.join(".local")
                .join("share")
                .join("applications")
                .join("antigravity.desktop"),
        ],
        ("windsurf", Os::Mac) => vec![
            PathBuf::from("/Applications/Windsurf.app"),
            home.join("Applications").join("Windsurf.app"),
        ],
        ("windsurf", Os::Windows) => lad
            .map(|l| vec![l.join("Programs").join("Windsurf")])
            .unwrap_or_default(),
        ("windsurf", Os::Linux) => vec![
            PathBuf::from("/usr/bin/windsurf"),
            PathBuf::from("/usr/share/windsurf"),
            PathBuf::from("/opt/windsurf"),
            PathBuf::from("/usr/share/applications/windsurf.desktop"),
            home.join(".local")
                .join("share")
                .join("applications")
                .join("windsurf.desktop"),
        ],
        ("vscode", Os::Mac) => vec![
            PathBuf::from("/Applications/Visual Studio Code.app"),
            home.join("Applications").join("Visual Studio Code.app"),
        ],
        ("vscode", Os::Windows) => {
            let mut v = vec![PathBuf::from("C:\\Program Files\\Microsoft VS Code")];
            if let Some(l) = lad {
                v.push(l.join("Programs").join("Microsoft VS Code"));
            }
            v
        }
        ("vscode", Os::Linux) => vec![
            PathBuf::from("/usr/share/code"),
            PathBuf::from("/usr/bin/code"),
            PathBuf::from("/opt/visual-studio-code"),
            PathBuf::from("/snap/code"),
        ],
        _ => Vec::new(),
    }
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

    // Write to temp file, then atomically rename into place — this function
    // overwrites OTHER applications' config files, and a crash/full-disk mid-write
    // would corrupt them. Mirroring appimage_stable_copy's safety pattern.
    let tmp = PathBuf::from(format!("{}.tmp", path.display()));
    if let Err(e) = std::fs::write(&tmp, &text).map_err(|e| e.to_string()) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    if let Err(e) = std::fs::rename(&tmp, path).map_err(|e| e.to_string()) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    Ok(())
}

/// Return `true` when any app-presence marker for the client exists.
fn is_client_available(client_id: &str, roots: &Roots) -> bool {
    client_app_markers(client_id, roots)
        .iter()
        .any(|p| p.exists())
}

/// `(installed, stale)`: installed when the `linxiv` entry exists in the
/// client's current config file (read live, never cached); stale when its
/// recorded command is an absolute path that no longer exists on disk —
/// e.g. the deleted Python-era PyInstaller sidecar
/// (`<resources>/binaries/linxiv-mcp-<triple>`), a dead AppImage mount, or a
/// moved dev checkout. A non-absolute command (resolved via the client's PATH,
/// or a bare name) is assumed live.
fn registration_state(path: &Path, key: &str) -> (bool, bool) {
    if !path.exists() {
        return (false, false);
    }
    let config = match read_mcp_config(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "[linxiv] registration_state: failed to read config at {}: {}",
                path.display(),
                e
            );
            return (false, false);
        }
    };
    let Some(entry) = config.get(key).and_then(|s| s.get("linxiv")) else {
        return (false, false);
    };
    let stale = match entry.get("command").and_then(|c| c.as_str()) {
        Some(cmd) if cmd.trim().is_empty() => true,
        Some(cmd) => Path::new(cmd).is_absolute() && !Path::new(cmd).exists(),
        // An entry without a runnable command cannot work.
        None => true,
    };
    (true, stale)
}

// ─────────────────────────────────────────────────────────────────────────────
// MCP commands
// ─────────────────────────────────────────────────────────────────────────────

/// Return all supported MCP clients with their current install/available status.
#[tauri::command]
pub fn list_mcp_clients() -> Vec<MpcClientStatus> {
    let roots = match Roots::current() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[linxiv] list_mcp_clients: {e}, marking all clients unavailable");
            return MCP_CLIENTS
                .iter()
                .map(|(id, name)| MpcClientStatus {
                    id: id.to_string(),
                    name: name.to_string(),
                    installed: false,
                    available: false,
                    stale: false,
                })
                .collect();
        }
    };

    MCP_CLIENTS
        .iter()
        .map(|(id, name)| {
            let (installed, stale) = if *id == "antigravity" {
                // Check both new and legacy paths, preferring whichever has a linxiv entry.
                antigravity_config_paths(&roots)
                    .iter()
                    .find_map(|p| {
                        let (inst, st) = registration_state(p, servers_key(id));
                        if inst {
                            Some((inst, st))
                        } else {
                            None
                        }
                    })
                    .unwrap_or((false, false))
            } else {
                mcp_config_path_in(id, &roots)
                    .map(|p| registration_state(&p, servers_key(id)))
                    .unwrap_or((false, false))
            };

            MpcClientStatus {
                id: id.to_string(),
                name: name.to_string(),
                installed,
                available: is_client_available(id, &roots),
                stale,
            }
        })
        .collect()
}

/// Register linxiv's MCP server in a client's config file.
///
/// Existing `mcpServers` entries are preserved; only the `"linxiv"` key is
/// added or overwritten.
///
/// Dev builds refuse unless LINXIV_DEV_INSTALL=1 is set.
#[tauri::command]
pub fn install_mcp(app: AppHandle, client_id: String) -> Result<(), String> {
    dev_install_guard(std::env::var("LINXIV_DEV_INSTALL").ok().as_deref())?;

    let binary = resolve_install_sidecar(&app, "linxiv-mcp")?;
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;

    let config_path = mcp_config_path(&client_id)?;
    let mut config = read_mcp_config(&config_path)?;

    let key = servers_key(&client_id);
    let servers = config
        .as_object_mut()
        .ok_or("Config root is not a JSON object")?
        .entry(key)
        .or_insert_with(|| serde_json::json!({}));

    let servers_obj = servers
        .as_object_mut()
        .ok_or_else(|| format!("{key} is not a JSON object"))?;

    let mut entry = serde_json::json!({
        "command": binary.to_string_lossy(),
        "args": [],
        "env": {
            "LINXIV_DATA_DIR": data_dir.to_string_lossy()
        }
    });
    // VS Code's mcp.json requires an explicit transport type per entry.
    if client_id == "vscode" {
        entry["type"] = serde_json::json!("stdio");
    }
    servers_obj.insert("linxiv".to_string(), entry);

    write_mcp_config(&config_path, &config)
}

/// In dev builds installing would persist the repo-local staged sidecar path
/// into user configs/shims — a path that dies with the checkout. Refuse unless
/// explicitly opted in to the value "1"; release builds pass through.
fn dev_install_guard(override_value: Option<&str>) -> Result<(), String> {
    #[cfg(debug_assertions)]
    if override_value != Some("1") {
        return Err(
            "This is a dev build: installing would register the repo-local dev \
             binary path. Set LINXIV_DEV_INSTALL=1 to override, or use an \
             installed release build."
                .to_string(),
        );
    }
    Ok(())
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
        .and_then(|o| o.get_mut(servers_key(&client_id)))
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
        Ok(p) => registration_state(&p, servers_key(&client_id)).0,
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
fn decode_utf16le(bytes: &[u8]) -> String {
    let utf16: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();
    String::from_utf16_lossy(&utf16)
        .trim_end_matches('\0')
        .to_string()
}

#[cfg(target_os = "windows")]
fn encode_utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(|u| u.to_le_bytes())
        .collect()
}

#[cfg(target_os = "windows")]
fn windows_path_add(dir: &str) -> Result<(), String> {
    use winreg::enums::{RegType, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::{RegKey, RegValue};

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let env = hkcu
        .open_subkey_with_flags("Environment", KEY_READ | KEY_WRITE)
        .map_err(|e| e.to_string())?;

    // Preserve existing registry type, defaulting to REG_EXPAND_SZ for new keys.
    let (current_path, vtype) = match env.get_raw_value("Path") {
        Ok(raw) => (decode_utf16le(&raw.bytes), raw.vtype),
        Err(_) => (String::new(), RegType::REG_EXPAND_SZ),
    };

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
    env.set_raw_value(
        "Path",
        &RegValue {
            vtype,
            bytes: encode_utf16le(&new_path),
        },
    )
    .map_err(|e| e.to_string())
}

#[cfg(target_os = "windows")]
fn windows_path_remove(dir: &str) -> Result<(), String> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::{RegKey, RegValue};

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let env = hkcu
        .open_subkey_with_flags("Environment", KEY_READ | KEY_WRITE)
        .map_err(|e| e.to_string())?;

    // Preserve existing registry type.
    let (current_path, vtype) = match env.get_raw_value("Path") {
        Ok(raw) => (decode_utf16le(&raw.bytes), raw.vtype),
        Err(_) => return Ok(()),
    };

    let new_path: Vec<&str> = current_path
        .split(';')
        .filter(|e| !e.eq_ignore_ascii_case(dir))
        .collect();

    env.set_raw_value(
        "Path",
        &RegValue {
            vtype,
            bytes: encode_utf16le(&new_path.join(";")),
        },
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots(os: Os) -> Roots {
        Roots {
            os,
            home: PathBuf::from("/home/u"),
            appdata: Some(PathBuf::from("/appdata")),
            local_appdata: Some(PathBuf::from("/localappdata")),
        }
    }

    #[test]
    fn config_paths_per_platform() {
        let cases: &[(&str, Os, &str)] = &[
            (
                "claude",
                Os::Linux,
                "/home/u/.config/Claude/claude_desktop_config.json",
            ),
            (
                "claude",
                Os::Mac,
                "/home/u/Library/Application Support/Claude/claude_desktop_config.json",
            ),
            (
                "claude",
                Os::Windows,
                "/appdata/Claude/claude_desktop_config.json",
            ),
            ("claude-code", Os::Linux, "/home/u/.claude.json"),
            ("claude-code", Os::Mac, "/home/u/.claude.json"),
            ("claude-code", Os::Windows, "/home/u/.claude.json"),
            ("cursor", Os::Linux, "/home/u/.cursor/mcp.json"),
            ("cursor", Os::Mac, "/home/u/.cursor/mcp.json"),
            ("cursor", Os::Windows, "/home/u/.cursor/mcp.json"),
            (
                "antigravity",
                Os::Linux,
                "/home/u/.gemini/config/mcp_config.json",
            ),
            (
                "antigravity",
                Os::Mac,
                "/home/u/.gemini/config/mcp_config.json",
            ),
            (
                "antigravity",
                Os::Windows,
                "/home/u/.gemini/config/mcp_config.json",
            ),
            (
                "windsurf",
                Os::Linux,
                "/home/u/.codeium/windsurf/mcp_config.json",
            ),
            (
                "windsurf",
                Os::Mac,
                "/home/u/.codeium/windsurf/mcp_config.json",
            ),
            (
                "windsurf",
                Os::Windows,
                "/home/u/.codeium/windsurf/mcp_config.json",
            ),
            ("vscode", Os::Linux, "/home/u/.config/Code/User/mcp.json"),
            (
                "vscode",
                Os::Mac,
                "/home/u/Library/Application Support/Code/User/mcp.json",
            ),
            ("vscode", Os::Windows, "/appdata/Code/User/mcp.json"),
        ];
        for (id, os, want) in cases {
            let got = mcp_config_path_in(id, &roots(*os)).unwrap();
            assert_eq!(got, PathBuf::from(want), "client={id} os={os:?}");
        }
    }

    #[test]
    fn windows_appdata_required_only_where_used() {
        let bare = Roots {
            os: Os::Windows,
            home: PathBuf::from("/home/u"),
            appdata: None,
            local_appdata: None,
        };
        assert!(mcp_config_path_in("claude", &bare).is_err());
        assert!(mcp_config_path_in("vscode", &bare).is_err());
        assert!(mcp_config_path_in("cursor", &bare).is_ok());
        // Missing LOCALAPPDATA yields no markers, not a panic.
        assert!(client_app_markers("cursor", &bare).is_empty());
    }

    #[test]
    fn unknown_client_rejected() {
        assert!(mcp_config_path_in("nope", &roots(Os::Linux)).is_err());
    }

    #[test]
    fn vscode_uses_servers_key() {
        assert_eq!(servers_key("vscode"), "servers");
        assert_eq!(servers_key("cursor"), "mcpServers");
        assert_eq!(servers_key("claude"), "mcpServers");
    }

    #[test]
    fn every_client_has_markers_on_every_os() {
        for (id, _) in MCP_CLIENTS {
            for os in [Os::Linux, Os::Mac, Os::Windows] {
                assert!(
                    !client_app_markers(id, &roots(os)).is_empty(),
                    "client={id} os={os:?}"
                );
            }
        }
    }

    #[test]
    fn registration_state_detects_missing_and_stale() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("mcp.json");

        // No config file at all.
        assert_eq!(registration_state(&cfg, "mcpServers"), (false, false));

        // Registered against a live binary (this test executable).
        let me = std::env::current_exe().unwrap();
        let write = |v: serde_json::Value| std::fs::write(&cfg, v.to_string()).unwrap();
        write(serde_json::json!({
            "mcpServers": { "linxiv": { "command": me.to_string_lossy(), "args": [] } }
        }));
        assert_eq!(registration_state(&cfg, "mcpServers"), (true, false));

        // Registered against a deleted binary (Python-era sidecar shape).
        let gone = dir
            .path()
            .join("binaries")
            .join("linxiv-mcp-x86_64-unknown-linux-gnu");
        write(serde_json::json!({
            "mcpServers": { "linxiv": { "command": gone.to_string_lossy() } }
        }));
        assert_eq!(registration_state(&cfg, "mcpServers"), (true, true));

        // Bare command name resolves via PATH; not checkable, assumed live.
        write(serde_json::json!({
            "mcpServers": { "linxiv": { "command": "linxiv-mcp" } }
        }));
        assert_eq!(registration_state(&cfg, "mcpServers"), (true, false));

        // Entry with no command cannot run.
        write(serde_json::json!({ "mcpServers": { "linxiv": { "args": [] } } }));
        assert_eq!(registration_state(&cfg, "mcpServers"), (true, true));

        // VS Code shape lives under "servers", invisible under "mcpServers".
        write(serde_json::json!({
            "servers": { "linxiv": { "type": "stdio", "command": gone.to_string_lossy() } }
        }));
        assert_eq!(registration_state(&cfg, "servers"), (true, true));
        assert_eq!(registration_state(&cfg, "mcpServers"), (false, false));
    }

    #[test]
    fn dev_install_guard_rejects_unset_override() {
        #[cfg(debug_assertions)]
        {
            assert!(dev_install_guard(None).is_err());
        }
        #[cfg(not(debug_assertions))]
        {
            // Release builds pass through.
            assert!(dev_install_guard(None).is_ok());
        }
    }

    #[test]
    fn dev_install_guard_rejects_falsy_value() {
        #[cfg(debug_assertions)]
        {
            assert!(dev_install_guard(Some("0")).is_err());
        }
    }
}
