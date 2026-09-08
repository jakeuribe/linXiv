use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Resolve a bundled sidecar binary for installation. Install-time only: the
/// AppImage branch mutates the filesystem — not a pure path lookup.
/// Dev: `<src-tauri>/binaries/<name>-<triple>`; release: next to the main
/// executable with the triple stripped; AppImage: a durable copy under
/// `app_data_dir()/bin` (goes stale after an update until install is re-run).
fn resolve_install_sidecar(app: &AppHandle, name: &str) -> Result<PathBuf, String> {
    // `app` is only used in release mode (AppImage data dir); silence the
    // unused-variable warning in dev where resolution is purely path-based.
    #[cfg(debug_assertions)]
    let _ = app;

    #[cfg(debug_assertions)]
    let path = {
        // Dev binaries keep the triple suffix (see scripts/stage_sidecar.py).
        let triple = tauri::utils::platform::target_triple().map_err(|e| e.to_string())?;
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

/// Copy an ephemeral AppImage-mount sidecar to `app_data_dir()/bin/<name>`,
/// executable, overwriting any prior copy. Never executes off Linux.
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

/// CLI shim path — single source of truth for is/install/uninstall_cli:
/// symlink `~/.local/bin/linxiv`, or `%LOCALAPPDATA%\Programs\linxiv\linxiv.bat` on Windows.
fn cli_shim_path() -> Result<PathBuf, String> {
    #[cfg(not(target_os = "windows"))]
    {
        let home = std::env::home_dir().ok_or("Could not determine home directory")?;
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

/// Check whether the shim from `install_cli` is present. Deliberately not a
/// PATH lookup: deb/rpm ship `/usr/bin/linxiv`, which made PATH report "installed".
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

/// Install the bundled `linxiv` CLI onto PATH: symlink on Linux/macOS, `.bat`
/// shim + user PATH registry entry on Windows. Dev builds refuse unless LINXIV_DEV_INSTALL=1.
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
// In-place update: deb/rpm/pacman
//
// The Tauri updater plugin (main.rs) only ever swaps an AppImage, a macOS
// app.tar.gz, or a Windows NSIS/MSI in place — it has no notion of a
// package-manager-owned install. Rather than standing up distro repositories
// (a separate hosting + package-signing project), a native install self-updates
// by downloading the matching asset straight off the GitHub release and
// installing it with `pkexec`, the same one-time privilege prompt a user
// would see running `dpkg -i`/`rpm -U` by hand.
// ─────────────────────────────────────────────────────────────────────────────

/// "deb", "rpm", "pacman", or `None` (AppImage, dev build, or non-Linux) —
/// asks the system package database whether it owns the running executable.
fn linux_package_kind() -> Option<&'static str> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let owned_by = |cmd: &str, args: &[&str]| {
        std::process::Command::new(cmd)
            .args(args)
            .output()
            .is_ok_and(|o| o.status.success())
    };
    if owned_by("dpkg", &["-S", &exe.to_string_lossy()]) {
        Some("deb")
    } else if owned_by("rpm", &["-qf", &exe.to_string_lossy()]) {
        Some("rpm")
    } else if owned_by("pacman", &["-Qo", &exe.to_string_lossy()]) {
        // Pacman packages cannot be replaced by Tauri's AppImage updater;
        // route them through the native package path as well.
        Some("pacman")
    } else {
        None
    }
}

/// JS-facing: which package manager (if any) owns this install, so the UI can
/// pick native-package update vs. the Tauri updater. Blocking probe, run off the main thread.
#[tauri::command]
pub async fn get_linux_package_kind() -> Option<String> {
    tokio::task::spawn_blocking(|| linux_package_kind().map(str::to_string))
        .await
        .ok()
        .flatten()
}

/// Asset download hosts trusted for `apply_linux_package_update` — defense in
/// depth on top of the fact that the URL is no longer caller-supplied (see
/// `resolve_release_asset` below): it comes from our own GET to the pinned
/// `linxiv-dev/linXiv` repo's release, not from the webview. Not covered by
/// `tauri.conf.json`'s CSP — CSP only gates the webview's own fetch/XHR, and
/// this download runs in Rust via `reqwest`, which never sees it. Only checks
/// the first hop (reqwest follows redirects by default); `verify_digest`
/// below is the control that actually matters for what ends up on disk.
const ALLOWED_ASSET_HOSTS: [&str; 3] = [
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

fn is_allowed_asset_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    parsed.scheme() == "https"
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed
            .host_str()
            .is_some_and(|h| ALLOWED_ASSET_HOSTS.contains(&h))
}

/// Upper bound on a downloaded native package — generous for a desktop app,
/// just enough to refuse an absurd/misconfigured response before install.
const MAX_UPDATE_PACKAGE_BYTES: u64 = 300 * 1024 * 1024;

#[derive(serde::Deserialize)]
struct GhReleaseAsset {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
}

#[derive(serde::Deserialize)]
struct GhRelease {
    assets: Vec<GhReleaseAsset>,
}

/// Resolve the `.deb`/`.rpm` asset to install from the pinned repo's latest
/// release — fetched here, not trusted from the webview. `apply_linux_
/// package_update` is a root-privileged install; letting the caller pass in
/// the URL (and a matching digest) would let any JS running in the webview
/// point it at an arbitrary GitHub-hosted asset (the app renders untrusted
/// LaTeX/abstracts, so webview JS execution is an in-scope threat, not a
/// hypothetical one) and get it installed as root.
async fn resolve_release_asset(
    client: &reqwest::Client,
    kind: &str,
) -> Result<GhReleaseAsset, String> {
    let release: GhRelease = client
        .get("https://api.github.com/repos/linxiv-dev/linXiv/releases/latest")
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "linXiv-updater")
        .send()
        .await
        .map_err(|e| format!("Could not reach GitHub: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Unexpected response from GitHub: {e}"))?;
    let arch = arch_token(std::env::consts::ARCH, kind);
    release
        .assets
        .into_iter()
        .find(|a| package_asset_matches(&a.name, kind, arch))
        .ok_or_else(|| format!("No {arch} {kind} asset found on the latest release"))
}

fn package_asset_matches(name: &str, kind: &str, arch: &str) -> bool {
    let suffix = match kind {
        "pacman" => ".pkg.tar.zst",
        other => return name.ends_with(&format!(".{other}")) && name.contains(arch),
    };
    name.starts_with("linxiv-") && name.ends_with(suffix) && name.contains(arch)
}

/// release.yml's asset naming: rpm carries the raw Rust arch ("x86_64",
/// "aarch64", ...), deb uses Debian's names ("amd64", "arm64").
fn arch_token<'a>(rust_arch: &'a str, kind: &str) -> &'a str {
    match (rust_arch, kind) {
        ("x86_64", "deb") => "amd64",
        ("aarch64", "deb") => "arm64",
        (other, _) => other,
    }
}

/// `dpkg -i`/`rpm -U`/`pacman -U` don't check a signature on what they install,
/// and native package assets aren't covered by `createUpdaterArtifacts`'s minisign
/// signing (that only signs the AppImage/app.tar.gz/NSIS-MSI artifacts) — so
/// unlike that path, this one has no real code-signing: the sha256 checked
/// here comes from the same GitHub API response as the download URL, so it
/// catches transit/CDN corruption but not a forged API response or a
/// malicious release. TLS to GitHub is the actual trust boundary.
fn verify_digest(bytes: &[u8], expected: &str) -> Result<(), String> {
    let Some(hex) = expected.strip_prefix("sha256:") else {
        return Err(format!("Unrecognized digest format: {expected}"));
    };
    use sha2::{Digest, Sha256};
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual.eq_ignore_ascii_case(hex) {
        Ok(())
    } else {
        Err("Downloaded package does not match the release's checksum".to_string())
    }
}

/// Download the `.deb`/`.rpm` release asset and install it over the running
/// app via `pkexec`. The caller relaunches on success (see `updates.ts`).
#[tauri::command]
pub async fn apply_linux_package_update() -> Result<(), String> {
    let kind = tokio::task::spawn_blocking(linux_package_kind)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("Not a supported native package install")?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;
    let asset = resolve_release_asset(&client, kind).await?;
    if !is_allowed_asset_url(&asset.browser_download_url) {
        return Err("Refusing to download from an untrusted host".to_string());
    }

    eprintln!(
        "[linxiv] apply_linux_package_update: downloading {}",
        asset.browser_download_url
    );
    use futures_util::StreamExt;
    let mut stream = client
        .get(&asset.browser_download_url)
        .send()
        .await
        .map_err(|e| format!("Download failed: {e}"))?
        .bytes_stream();
    // Bounded by running total as chunks arrive, not by trusting
    // Content-Length (absent on a chunked response) or buffering an
    // unbounded body before checking its length.
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Download failed: {e}"))?;
        if bytes.len() + chunk.len() > MAX_UPDATE_PACKAGE_BYTES as usize {
            return Err("Release asset is larger than expected; refusing to install".to_string());
        }
        bytes.extend_from_slice(&chunk);
    }

    let expected_digest = asset
        .digest
        .as_deref()
        .ok_or("Release asset has no checksum on file; refusing to install")?;
    verify_digest(&bytes, expected_digest)?;

    // Writing the package and running pkexec both block (a real temp file,
    // then the interactive polkit prompt) — off the async runtime so a slow
    // password prompt can't stall other in-flight commands.
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        use std::io::Write;
        // O_EXCL unique temp path in the sticky-bit temp dir: a predictable
        // shared-dir path could be symlink-swapped by another local user
        // between write and pkexec's read; this one can't be pre-created or
        // guessed.
        let suffix = if kind == "pacman" {
            "pkg.tar.zst"
        } else {
            kind
        };
        let mut tmp = tempfile::Builder::new()
            .prefix("linxiv-update-")
            .suffix(&format!(".{suffix}"))
            .tempfile()
            .map_err(|e| format!("Could not create temp file: {e}"))?;
        tmp.write_all(&bytes)
            .and_then(|_| tmp.flush())
            .map_err(|e| format!("Could not write update package: {e}"))?;

        let (installer, install_args): (&str, &[&str]) = match kind {
            "deb" => ("dpkg", &["-i"]),
            "rpm" => ("rpm", &["-U"]),
            "pacman" => ("pacman", &["-U", "--noconfirm"]),
            _ => return Err(format!("Unsupported package manager: {kind}")),
        };
        let path_str = tmp.path().to_string_lossy().to_string();
        eprintln!(
            "[linxiv] apply_linux_package_update: pkexec {installer} {} {path_str}",
            install_args.join(" ")
        );
        let status = std::process::Command::new("pkexec")
            .arg(installer)
            .args(install_args)
            .arg(&path_str)
            .status()
            .map_err(|e| format!("Could not launch pkexec: {e}"))?;
        // tmp stays in scope (not dropped/deleted) until here.
        if !status.success() {
            return Err(format!("{installer} exited with {status}"));
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("Install task panicked: {e}"))??;

    eprintln!("[linxiv] apply_linux_package_update: installed, ready to relaunch");
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
    /// `true` when the registered command no longer exists on disk (e.g. a
    /// since-deleted sidecar) and the entry needs a reinstall.
    pub stale: bool,
    /// `true` when the client's config file exists but could not be parsed as
    /// JSON, so `installed`/`stale` could not be determined and default to
    /// `false`. Distinct from "genuinely not installed" — the config needs
    /// manual repair.
    pub config_error: bool,
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
            home: std::env::home_dir().ok_or("Could not determine home directory")?,
            appdata: std::env::var("APPDATA").ok().map(PathBuf::from),
            local_appdata: std::env::var("LOCALAPPDATA").ok().map(PathBuf::from),
        })
    }
}

/// Key under which a client lists MCP servers: VS Code's `mcp.json` uses
/// `servers` (with a `type` per entry); the rest use the reference `mcpServers` shape.
fn servers_key(client_id: &str) -> &'static str {
    match client_id {
        "vscode" => "servers",
        _ => "mcpServers",
    }
}

/// Path of the client's user-level MCP config file; Antigravity 2.0 shares
/// Gemini's `~/.gemini/config/mcp_config.json`.
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

/// `mcp_config_path_in` for the current machine; Antigravity has two candidate
/// paths (see `antigravity_target_path`), every other client one canonical path.
fn mcp_config_path(client_id: &str) -> Result<PathBuf, String> {
    let roots = Roots::current()?;
    if client_id == "antigravity" {
        return Ok(antigravity_target_path(&roots));
    }
    mcp_config_path_in(client_id, &roots)
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

/// Which Antigravity config path install/uninstall targets: whichever already
/// has linxiv registered, else whichever config dir exists on disk, else the modern default.
// ponytail: the dir-exists fallback can't tell "Antigravity 2.0 created this"
// from "the Gemini CLI created this" — no signal distinguishes them. Fine
// until a bug report shows that combination in the wild.
fn antigravity_target_path(roots: &Roots) -> PathBuf {
    let paths = antigravity_config_paths(roots);
    let key = servers_key("antigravity");
    paths
        .iter()
        .find(|p| registration_state(p, key).0)
        .or_else(|| {
            paths
                .iter()
                .find(|p| p.parent().is_some_and(|d| d.exists()))
        })
        .cloned()
        .unwrap_or_else(|| paths[0].clone())
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
        // Fixed path list; an unintegrated AppImage is undetectable.
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

/// `(installed, stale, config_error)`: installed when the `linxiv` entry is in
/// the config (read live, never cached); stale when its command is an absolute
/// path missing on disk (since-deleted sidecar, dead AppImage mount, moved dev
/// checkout) — non-absolute commands are assumed live. `config_error` flags an
/// unparseable config, kept distinct from "never installed" so a broken file
/// surfaces as something to repair by hand.
fn registration_state(path: &Path, key: &str) -> (bool, bool, bool) {
    if !path.exists() {
        return (false, false, false);
    }
    let config = match read_mcp_config(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "[linxiv] registration_state: failed to read config at {}: {}",
                path.display(),
                e
            );
            return (false, false, true);
        }
    };
    let Some(entry) = config.get(key).and_then(|s| s.get("linxiv")) else {
        return (false, false, false);
    };
    let stale = match entry.get("command").and_then(|c| c.as_str()) {
        Some(cmd) if cmd.trim().is_empty() => true,
        Some(cmd) => Path::new(cmd).is_absolute() && !Path::new(cmd).exists(),
        // An entry without a runnable command cannot work.
        None => true,
    };
    (true, stale, false)
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
                    config_error: false,
                })
                .collect();
        }
    };

    MCP_CLIENTS
        .iter()
        .map(|(id, name)| {
            let (installed, stale, config_error) = if *id == "antigravity" {
                // Check both new and legacy paths, preferring whichever has a linxiv
                // entry; if none is installed but any path had a broken config,
                // surface that instead of reporting a flat "not installed".
                let results: Vec<(bool, bool, bool)> = antigravity_config_paths(&roots)
                    .iter()
                    .map(|p| registration_state(p, servers_key(id)))
                    .collect();
                results
                    .iter()
                    .find(|(inst, _, _)| *inst)
                    .copied()
                    .unwrap_or_else(|| (false, false, results.iter().any(|(_, _, err)| *err)))
            } else {
                mcp_config_path_in(id, &roots)
                    .map(|p| registration_state(&p, servers_key(id)))
                    .unwrap_or((false, false, false))
            };

            MpcClientStatus {
                id: id.to_string(),
                name: name.to_string(),
                installed,
                available: is_client_available(id, &roots),
                stale,
                config_error,
            }
        })
        .collect()
}

/// Register linxiv's MCP server in a client's config; only the `"linxiv"` key
/// is added or overwritten. Dev builds refuse unless LINXIV_DEV_INSTALL=1.
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

/// Dev builds would persist the repo-local staged sidecar path — dead once the
/// checkout moves — so refuse unless the override is exactly "1"; release builds pass through.
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
fn windows_path_add(dir: &str) -> Result<(), String> {
    use winreg::enums::{RegType, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::types::ToRegValue;
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
            bytes: new_path.to_reg_value().bytes,
        },
    )
    .map_err(|e| e.to_string())
}

#[cfg(target_os = "windows")]
fn windows_path_remove(dir: &str) -> Result<(), String> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::types::ToRegValue;
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
            bytes: new_path.join(";").to_reg_value().bytes,
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
        assert_eq!(
            registration_state(&cfg, "mcpServers"),
            (false, false, false)
        );

        // Registered against a live binary (this test executable).
        let me = std::env::current_exe().unwrap();
        let write = |v: serde_json::Value| std::fs::write(&cfg, v.to_string()).unwrap();
        write(serde_json::json!({
            "mcpServers": { "linxiv": { "command": me.to_string_lossy(), "args": [] } }
        }));
        assert_eq!(registration_state(&cfg, "mcpServers"), (true, false, false));

        // Registered against a since-deleted binary (old sidecar path shape).
        let gone = dir
            .path()
            .join("binaries")
            .join("linxiv-mcp-x86_64-unknown-linux-gnu");
        write(serde_json::json!({
            "mcpServers": { "linxiv": { "command": gone.to_string_lossy() } }
        }));
        assert_eq!(registration_state(&cfg, "mcpServers"), (true, true, false));

        // Bare command name resolves via PATH; not checkable, assumed live.
        write(serde_json::json!({
            "mcpServers": { "linxiv": { "command": "linxiv-mcp" } }
        }));
        assert_eq!(registration_state(&cfg, "mcpServers"), (true, false, false));

        // Entry with no command cannot run.
        write(serde_json::json!({ "mcpServers": { "linxiv": { "args": [] } } }));
        assert_eq!(registration_state(&cfg, "mcpServers"), (true, true, false));

        // VS Code shape lives under "servers", invisible under "mcpServers".
        write(serde_json::json!({
            "servers": { "linxiv": { "type": "stdio", "command": gone.to_string_lossy() } }
        }));
        assert_eq!(registration_state(&cfg, "servers"), (true, true, false));
        assert_eq!(
            registration_state(&cfg, "mcpServers"),
            (false, false, false)
        );
    }

    #[test]
    fn antigravity_target_path_prefers_existing_registration_then_existing_dir() {
        let dir = tempfile::tempdir().unwrap();
        let mut roots = roots(Os::Linux);
        roots.home = dir.path().to_path_buf();
        let paths = antigravity_config_paths(&roots);
        let (new_path, legacy_path) = (&paths[0], &paths[1]);

        // Neither path's dir exists yet: default to the new canonical path.
        assert_eq!(antigravity_target_path(&roots), *new_path);

        // Only the legacy dir exists (old Codeium-era app, never ran the new
        // Gemini-based one): install must target the path that app reads.
        std::fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();
        assert_eq!(antigravity_target_path(&roots), *legacy_path);

        // Both dirs exist but only legacy has linxiv registered: reinstall/
        // uninstall must keep targeting the one with the real entry.
        std::fs::create_dir_all(new_path.parent().unwrap()).unwrap();
        std::fs::write(
            legacy_path,
            serde_json::json!({ "mcpServers": { "linxiv": { "command": "linxiv-mcp" } } })
                .to_string(),
        )
        .unwrap();
        assert_eq!(antigravity_target_path(&roots), *legacy_path);

        // Both registered: new path wins (checked first, matches list_mcp_clients).
        std::fs::write(
            new_path,
            serde_json::json!({ "mcpServers": { "linxiv": { "command": "linxiv-mcp" } } })
                .to_string(),
        )
        .unwrap();
        assert_eq!(antigravity_target_path(&roots), *new_path);
    }

    #[test]
    fn registration_state_reports_config_error_not_plain_missing() {
        // A client that IS installed but whose config got hand-edited into
        // invalid JSON must not be reported identically to "never installed" —
        // that hides a real problem the user needs to go fix.
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("mcp.json");
        std::fs::write(&cfg, "{ not valid json").unwrap();
        assert_eq!(registration_state(&cfg, "mcpServers"), (false, false, true));
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

    #[test]
    fn asset_url_allowlist() {
        let cases: &[(&str, bool)] = &[
            (
                "https://github.com/linxiv-dev/linXiv/releases/download/v1/x.deb",
                true,
            ),
            ("https://objects.githubusercontent.com/abc", true),
            ("https://release-assets.githubusercontent.com/abc", true),
            ("http://github.com/x.deb", false), // not https
            ("https://evil.com/x.deb", false),  // not allowlisted
            ("https://github.com:x@evil.com/x.deb", false), // userinfo host-confusion
            ("https://github.com@evil.com/x.deb", false), // userinfo, no port
            ("not a url", false),
        ];
        for (url, expected) in cases {
            assert_eq!(is_allowed_asset_url(url), *expected, "url: {url}");
        }
    }

    #[test]
    fn digest_verification() {
        let bytes = b"hello world";
        // Round-trip against our own hasher rather than a hand-typed hex
        // digest, so the test can't just have the wrong constant.
        use sha2::{Digest, Sha256};
        let matching = format!("sha256:{:x}", Sha256::digest(bytes));
        assert!(verify_digest(bytes, &matching).is_ok());

        let wrong = "sha256:0000000000000000000000000000000000000000000000000000000000000000";
        assert!(verify_digest(bytes, wrong).is_err());
        assert!(verify_digest(bytes, "md5:abc").is_err());
    }

    #[test]
    fn arch_token_mapping() {
        let cases: &[(&str, &str, &str)] = &[
            ("x86_64", "deb", "amd64"),
            ("x86_64", "rpm", "x86_64"),
            ("aarch64", "deb", "arm64"),
            ("aarch64", "rpm", "aarch64"),
            ("x86_64", "pacman", "x86_64"),
        ];
        for (rust_arch, kind, expected) in cases {
            assert_eq!(arch_token(rust_arch, kind), *expected);
        }
    }

    #[test]
    fn native_package_asset_matching() {
        assert!(package_asset_matches(
            "linxiv-0.4.1-1-x86_64.pkg.tar.zst",
            "pacman",
            "x86_64"
        ));
        assert!(!package_asset_matches(
            "linxiv-0.4.1-1-aarch64.pkg.tar.zst",
            "pacman",
            "x86_64"
        ));
        assert!(package_asset_matches(
            "linXiv_0.4.1_amd64.deb",
            "deb",
            "amd64"
        ));
        assert!(package_asset_matches(
            "linXiv-0.4.1-1.x86_64.rpm",
            "rpm",
            "x86_64"
        ));
    }
}
