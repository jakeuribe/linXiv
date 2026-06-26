//! Runtime paths + user settings. Rust port of `config.py`, `storage/paths.py`,
//! and `user_settings.py`. Plan §5.8 + ADR-0014 (LINXIV_DATA_DIR is the single
//! source of truth) + D24 (data-dir parity with Tauri).

use std::env;
use std::path::PathBuf;

use directories::BaseDirs;
use serde_json::{Map, Value};

use crate::error::{CoreError, Result};

const ENV_DATA_DIR: &str = "LINXIV_DATA_DIR";
/// Must match `src-tauri/tauri.conf.json` "identifier" and `config.py` _APP_IDENTIFIER.
const APP_IDENTIFIER: &str = "com.linxiv.app";
const USER_SETTINGS_FILE: &str = "user_settings.json";

/// Bundled defaults, embedded at compile time from the canonical source file so the
/// Rust defaults can never drift from `formats/default_settings.json`.
const BUNDLED_DEFAULTS: &str = include_str!("../assets/default_settings.json");

fn map_io(e: std::io::Error) -> CoreError {
    CoreError::Internal(e.to_string())
}

fn map_json(e: serde_json::Error) -> CoreError {
    CoreError::Internal(e.to_string())
}

/// OS per-user app-data dir for `com.linxiv.app`, byte-matching Tauri's `app_data_dir()`
/// (which is `dirs::data_dir().join(identifier)`) on Linux/macOS/Windows. Used only when
/// LINXIV_DATA_DIR is unset (dev / CLI / MCP launched without Tauri). Never the repo.
//
// `BaseDirs::data_dir()` is the `directories`-crate equivalent of `dirs::data_dir()`:
//   Linux   $XDG_DATA_HOME or ~/.local/share
//   macOS   ~/Library/Application Support
//   Windows %APPDATA% (Roaming)
// then we append the identifier as a single path segment, exactly like Tauri.
fn _default_data_dir() -> PathBuf {
    BaseDirs::new()
        .expect("no home directory: cannot resolve the OS data dir")
        .data_dir()
        .join(APP_IDENTIFIER)
}

/// Runtime data dir (DB, PDFs, user settings, vaults). Resolved on every call so it tracks
/// LINXIV_DATA_DIR dynamically; falls back to the OS app-data dir when unset. Never the repo.
pub fn data_dir() -> PathBuf {
    match env::var_os(ENV_DATA_DIR) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => _default_data_dir(),
    }
}

/// Resolve, pin, and create the data dir. Call once at startup before any DB/PDF/vault access.
/// Writes the resolved path back to LINXIV_DATA_DIR so the value is stable for the process and
/// inherited by children (ADR-0014).
pub fn init_data_dir() -> Result<PathBuf> {
    let path = data_dir();
    // edition-2021: env::set_var is safe (becomes `unsafe` only under edition-2024).
    env::set_var(ENV_DATA_DIR, &path);
    std::fs::create_dir_all(&path).map_err(map_io)?;
    Ok(path)
}

// Path helpers — mirror storage/paths.py. Each resolves through data_dir() per call.
pub fn db_path() -> PathBuf {
    data_dir().join("papers.db")
}

pub fn pdf_dir() -> PathBuf {
    data_dir().join("pdfs")
}

/// Root of the embedded-editor LaTeX vaults (one subdir per editor project).
pub fn vault_dir() -> PathBuf {
    data_dir().join("vaults")
}

/// Legacy PDF location (`storage/paths.py::old_pdf_dir`) — only the blue→green
/// `migrate_db` path reads it (ported last, plan §5.3).
pub fn old_pdf_dir() -> PathBuf {
    data_dir().join("gui").join("pdfs")
}

/// Loads a dev `.env` (dev API keys) if present; a missing file is a silent no-op
/// (matching python-dotenv). Unlike Python's pinned `load_dotenv(ENV_PATH)` (repo
/// `.env`), dotenvy walks up from CWD — fine for `cargo run` in the repo; prod keys
/// come from the real environment / SQLite settings (D20).
pub fn load_dotenv() -> Result<()> {
    match dotenvy::dotenv() {
        Ok(_) => Ok(()),
        Err(e) if e.not_found() => Ok(()),
        Err(e) => Err(CoreError::Internal(e.to_string())),
    }
}

/// User settings: bundled defaults overlaid by the user's overrides (shallow merge), mirroring
/// `user_settings.py`. Only the overrides are persisted, never the defaults.
pub struct UserSettings {
    defaults: Map<String, Value>,
    overrides: Map<String, Value>,
}

impl UserSettings {
    /// Load overrides from `data_dir()/user_settings.json` over the bundled defaults.
    /// A missing user file yields no overrides (pure defaults).
    pub fn load() -> Result<Self> {
        let defaults = parse_obj(BUNDLED_DEFAULTS)?;
        let path = data_dir().join(USER_SETTINGS_FILE);
        let overrides = match std::fs::read_to_string(&path) {
            Ok(s) => parse_obj(&s)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Map::new(),
            Err(e) => return Err(map_io(e)),
        };
        Ok(Self { defaults, overrides })
    }

    /// Override value if set, else the bundled default. `None` if the key exists nowhere.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.overrides.get(key).or_else(|| self.defaults.get(key))
    }

    /// Shallow merge `{**defaults, **overrides}` — the effective settings.
    pub fn all(&self) -> Map<String, Value> {
        let mut merged = self.defaults.clone();
        for (k, v) in &self.overrides {
            merged.insert(k.clone(), v.clone());
        }
        merged
    }

    /// Set an override and persist immediately — write-through, matching
    /// `user_settings.py::set` (which calls `save()`). In-memory-only would
    /// silently drop persistence in the ported `update_setting`/CLI/API paths.
    pub fn set(&mut self, key: impl Into<String>, value: Value) -> Result<()> {
        self.overrides.insert(key.into(), value);
        self.save()
    }

    /// Persist only the overrides (pretty-printed, like the Python `json.dumps(indent=2)` writer).
    pub fn save(&self) -> Result<()> {
        let path = data_dir().join(USER_SETTINGS_FILE);
        let body = serde_json::to_string_pretty(&self.overrides).map_err(map_json)?;
        std::fs::write(path, body).map_err(map_io)
    }
}

fn parse_obj(s: &str) -> Result<Map<String, Value>> {
    serde_json::from_str(s).map_err(map_json)
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test: env-var mutation is process-global, so keep all data_dir-touching assertions
    // sequential in a single function to avoid races with the test runner's threads.
    #[test]
    fn data_dir_settings_roundtrip() {
        // Default (no env): byte-matches Tauri's app_data_dir leaf.
        env::remove_var(ENV_DATA_DIR);
        assert_eq!(
            data_dir().file_name().unwrap().to_str().unwrap(),
            APP_IDENTIFIER
        );
        // And equals the BaseDirs base + identifier (the exact Tauri form).
        let expect = BaseDirs::new().unwrap().data_dir().join(APP_IDENTIFIER);
        assert_eq!(data_dir(), expect);

        // Redirect to a scratch dir and init it.
        let scratch = env::temp_dir().join(format!("linxiv-cfg-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        env::set_var(ENV_DATA_DIR, &scratch);
        assert_eq!(data_dir(), scratch);
        let made = init_data_dir().unwrap();
        assert!(made.is_dir());
        assert_eq!(db_path(), scratch.join("papers.db"));
        assert_eq!(pdf_dir(), scratch.join("pdfs"));
        assert_eq!(vault_dir(), scratch.join("vaults"));

        // Defaults present; no user file yet.
        let s = UserSettings::load().unwrap();
        assert_eq!(s.get("pdf_save_limit_mb").unwrap().as_i64().unwrap(), 1024);
        assert_eq!(s.get("tex_rendering_enabled").unwrap().as_bool().unwrap(), true);
        assert!(s.get("nope").is_none());

        // Override + save persists ONLY the override, then reloads merged.
        let mut s = s;
        s.set("pdf_save_limit_mb", Value::from(42)).unwrap(); // write-through persists
        let raw: Map<String, Value> =
            serde_json::from_str(&std::fs::read_to_string(scratch.join(USER_SETTINGS_FILE)).unwrap())
                .unwrap();
        assert_eq!(raw.len(), 1); // only the override written, not the defaults
        assert_eq!(raw["pdf_save_limit_mb"], Value::from(42));

        let s = UserSettings::load().unwrap();
        assert_eq!(s.get("pdf_save_limit_mb").unwrap().as_i64().unwrap(), 42);
        // Untouched default still resolves through the merge.
        assert_eq!(s.all()["tex_rendering_enabled"].as_bool().unwrap(), true);

        let _ = std::fs::remove_dir_all(&scratch);
        env::remove_var(ENV_DATA_DIR);
    }
}
