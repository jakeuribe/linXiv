//! Runtime paths + user settings. Rust port of `config.py`, `storage/paths.py`,
//! and `user_settings.py`. Plan §5.8 + ADR-0014 (LINXIV_DATA_DIR is the single
//! source of truth) + D24 (data-dir parity with Tauri).

use std::env;
use std::path::PathBuf;

use directories::BaseDirs;
use serde_json::{Map, Value};

use crate::error::Result;

const ENV_DATA_DIR: &str = "LINXIV_DATA_DIR";
/// Must match `src-tauri/tauri.conf.json` "identifier" and `config.py` _APP_IDENTIFIER.
const APP_IDENTIFIER: &str = "com.linxiv.app";
const USER_SETTINGS_FILE: &str = "user_settings.json";

/// Bundled defaults, embedded at compile time from the canonical source file so the
/// Rust defaults can never drift from `formats/default_settings.json`.
const BUNDLED_DEFAULTS: &str = include_str!("../assets/default_settings.json");

/// Runtime data dir (DB, PDFs, user settings, vaults). Resolved on every call so it tracks
/// LINXIV_DATA_DIR dynamically; falls back to the OS app-data dir when unset. Never the repo.
///
/// The fallback is the OS per-user app-data dir for `com.linxiv.app`, byte-matching Tauri's
/// `app_data_dir()` (which is `dirs::data_dir().join(identifier)`) on Linux/macOS/Windows.
//
// `BaseDirs::data_dir()` is the `directories`-crate equivalent of `dirs::data_dir()`:
//   Linux   $XDG_DATA_HOME or ~/.local/share
//   macOS   ~/Library/Application Support
//   Windows %APPDATA% (Roaming)
// then we append the identifier as a single path segment, exactly like Tauri.
pub fn data_dir() -> PathBuf {
    match env::var_os(ENV_DATA_DIR) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => BaseDirs::new()
            .expect("no home directory: cannot resolve the OS data dir")
            .data_dir()
            .join(APP_IDENTIFIER),
    }
}

/// Resolve, pin, and create the data dir. Call once at startup before any DB/PDF/vault access.
/// Writes the resolved path back to LINXIV_DATA_DIR so the value is stable for the process and
/// inherited by children (ADR-0014).
pub fn init_data_dir() -> Result<PathBuf> {
    let path = data_dir();
    // edition-2021: env::set_var is safe (becomes `unsafe` only under edition-2024).
    env::set_var(ENV_DATA_DIR, &path);
    std::fs::create_dir_all(&path)?;
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

/// OpenAlex polite-pool address (`OPENALEX_MAILTO`).
pub fn openalex_mailto() -> String {
    mailto_setting("OPENALEX_MAILTO")
}

/// CrossRef polite-pool address (`CROSSREF_MAILTO`).
pub fn crossref_mailto() -> String {
    mailto_setting("CROSSREF_MAILTO")
}

/// A polite-pool address; CR/LF are stripped downstream in
/// `sources::http::polite_user_agent`.
///
/// The env var wins; a user-settings override is the fallback. Only the app sets the
/// env var (`PATCH /api/env`), and it sets it on its own process — so without this
/// fallback the CLI and MCP server, which run as separate processes, read the value
/// as empty no matter what was configured, and `settings update <KEY>` wrote a key
/// nothing ever read.
fn mailto_setting(key: &str) -> String {
    match std::env::var(key) {
        Ok(v) if !v.is_empty() => v,
        _ => UserSettings::load()
            .ok()
            .and_then(|s| s.get(key).and_then(Value::as_str).map(String::from))
            .unwrap_or_default(),
    }
}

/// User settings: bundled defaults overlaid by the user's overrides (shallow merge), mirroring
/// `user_settings.py`. Only the overrides are persisted, never the defaults.
pub struct UserSettings {
    defaults: &'static Map<String, Value>,
    overrides: Map<String, Value>,
}

/// The bundled defaults, parsed once per process. Callers hit this on every
/// settings read (feed polls, uploads, the full-text worker), so re-parsing the
/// embedded JSON each time was pure waste. The expect is safe: the JSON is
/// compiled in, so a parse failure is a build defect, not a runtime condition.
fn bundled_defaults() -> &'static Map<String, Value> {
    static DEFAULTS: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
    DEFAULTS.get_or_init(|| {
        parse_obj(BUNDLED_DEFAULTS).expect("bundled default_settings.json is a JSON object")
    })
}

impl UserSettings {
    /// Load overrides from `data_dir()/user_settings.json` over the bundled defaults.
    /// A missing user file yields no overrides (pure defaults).
    pub fn load() -> Result<Self> {
        let defaults = bundled_defaults();
        let path = data_dir().join(USER_SETTINGS_FILE);
        let overrides = match std::fs::read_to_string(&path) {
            Ok(s) => parse_obj(&s)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Map::new(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self {
            defaults,
            overrides,
        })
    }

    /// Override value if set, else the bundled default. `None` if the key exists nowhere.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.overrides.get(key).or_else(|| self.defaults.get(key))
    }

    /// Shallow merge `{**defaults, **overrides}` — the effective settings.
    pub fn all(&self) -> Map<String, Value> {
        let mut merged = (*self.defaults).clone();
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

    /// Set an override from a raw command-line/tool string: parsed as JSON when it
    /// is valid JSON, else stored verbatim as a string. Returns what was stored so
    /// the caller can echo it. The one home for the rule (`linxiv settings update`
    /// and MCP `update_setting` each used to spell it out).
    pub fn set_from_str(&mut self, key: impl Into<String>, raw: String) -> Result<Value> {
        let parsed = serde_json::from_str::<Value>(&raw).unwrap_or(Value::String(raw));
        self.set(key, parsed.clone())?;
        Ok(parsed)
    }

    /// Persist only the overrides (pretty-printed, like the Python `json.dumps(indent=2)` writer).
    pub fn save(&self) -> Result<()> {
        let path = data_dir().join(USER_SETTINGS_FILE);
        let body = serde_json::to_string_pretty(&self.overrides)?;
        std::fs::write(path, body)?;
        Ok(())
    }

    /// `pdf_save_limit_mb`, converted to bytes — the TOTAL-storage cap across all managed
    /// PDFs, enforced before every new PDF write by `service::paper_import::import_pdf` and
    /// `service::files::download_pdf`. Falls back to the bundled default (1024 MB) if the
    /// setting is missing or not a positive integer, so a hand-edited settings file can't
    /// silently disable the cap (saturating_mul keeps an absurd value from overflowing it away).
    pub fn pdf_save_limit_bytes(&self) -> u64 {
        let mb = self
            .get("pdf_save_limit_mb")
            .and_then(Value::as_i64)
            .filter(|&v| v > 0)
            .unwrap_or(1024) as u64;
        mb.saturating_mul(1024 * 1024)
    }

    /// `rss_cache_retention_days`: how many days `RSS_CACHE_ENTRY` rows are kept
    /// before pruning. Also floors `rss::prune_dismissed`'s VER cutoff -- a
    /// dismissal can't be forgotten before the cache entry it hides is gone.
    /// Falls back to 30 if missing or not a positive integer.
    ///
    /// TODO: prune_dismissed's cutoffs are hardcoded, not settings -- surface
    /// once there's a real need to tune them per-user.
    pub fn rss_cache_retention_days(&self) -> i64 {
        self.get("rss_cache_retention_days")
            .and_then(Value::as_i64)
            .filter(|&v| v > 0)
            .unwrap_or(30)
    }

    /// `pdf_import_verify_identity_enabled`: whether the PDF-metadata-first
    /// import short-circuit (`sources::pdf_metadata::resolve_from_extracted`)
    /// makes its one optional network lookup — fetching a text-scanned arXiv
    /// id/DOI candidate to confirm it before adopting it as dedupe identity.
    /// Off means PDF-only: no network call for identity, ever, even when a
    /// candidate id is sitting right there in the text. Falls back to `true`
    /// (verify) if missing or not a bool.
    pub fn pdf_import_verify_identity_enabled(&self) -> bool {
        self.get("pdf_import_verify_identity_enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }
}

fn parse_obj(s: &str) -> Result<Map<String, Value>> {
    Ok(serde_json::from_str(s)?)
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
        assert!(s.get("tex_rendering_enabled").unwrap().as_bool().unwrap());
        assert_eq!(s.rss_cache_retention_days(), 30);
        assert!(s.pdf_import_verify_identity_enabled()); // defaults to true
        assert!(s.get("nope").is_none());

        // Override + save persists ONLY the override, then reloads merged.
        let mut s = s;
        s.set("pdf_save_limit_mb", Value::from(42)).unwrap(); // write-through persists
        let raw: Map<String, Value> = serde_json::from_str(
            &std::fs::read_to_string(scratch.join(USER_SETTINGS_FILE)).unwrap(),
        )
        .unwrap();
        assert_eq!(raw.len(), 1); // only the override written, not the defaults
        assert_eq!(raw["pdf_save_limit_mb"], Value::from(42));

        // Override flips it off; falls back to true if the stored value isn't a bool.
        s.set("pdf_import_verify_identity_enabled", Value::from(false))
            .unwrap();
        assert!(!UserSettings::load()
            .unwrap()
            .pdf_import_verify_identity_enabled());
        s.set(
            "pdf_import_verify_identity_enabled",
            Value::from("not a bool"),
        )
        .unwrap();
        assert!(UserSettings::load()
            .unwrap()
            .pdf_import_verify_identity_enabled());

        let s = UserSettings::load().unwrap();
        assert_eq!(s.get("pdf_save_limit_mb").unwrap().as_i64().unwrap(), 42);
        // Untouched default still resolves through the merge.
        assert!(s.all()["tex_rendering_enabled"].as_bool().unwrap());

        // openalex_mailto: unset env falls back to the settings override, so the CLI
        // and MCP processes see what `settings update` wrote; a set env var wins.
        env::remove_var("OPENALEX_MAILTO");
        assert_eq!(openalex_mailto(), "");
        let mut s = s;
        s.set("OPENALEX_MAILTO", Value::from("settings@example.org"))
            .unwrap();
        assert_eq!(openalex_mailto(), "settings@example.org");
        env::set_var("OPENALEX_MAILTO", "env@example.org");
        assert_eq!(openalex_mailto(), "env@example.org");
        // Empty env var is treated as unset, not as an override of the settings value.
        env::set_var("OPENALEX_MAILTO", "");
        assert_eq!(openalex_mailto(), "settings@example.org");
        env::remove_var("OPENALEX_MAILTO");

        let _ = std::fs::remove_dir_all(&scratch);
        env::remove_var(ENV_DATA_DIR);
    }
}
