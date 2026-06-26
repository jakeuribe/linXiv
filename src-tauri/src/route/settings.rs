//! `/api/settings` routes — `api/app.py` 1052–1070. GET returns the flat user
//! settings object with the env keys CROSSREF_MAILTO/OPENALEX_MAILTO overlaid on
//! top (no wrapper key); PATCH applies a batch of `set(k, v)` updates. Core
//! binding mirrors `mcp/src/io_authors_misc.rs::{get_settings, update_setting}`.

use serde::Deserialize;
use serde_json::{json, Map, Value};

use linxiv_core::config::UserSettings;

use crate::route::{ApiError, ReqCtx};
use crate::state::AppState;

/// `_SETTINGS_ENV_KEYS` (app.py 1048-1049): env values merged into the GET body.
const SETTINGS_ENV_KEYS: [&str; 2] = ["CROSSREF_MAILTO", "OPENALEX_MAILTO"];

/// `_ALLOWED_ENV_KEYS` (app.py): the only keys `PATCH /api/env` may set.
const ALLOWED_ENV_KEYS: [&str; 4] =
    ["CROSSREF_MAILTO", "OPENALEX_MAILTO", "GEMINI_API_KEY", "OPENAI_API_KEY"];

pub(crate) async fn handle(_state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("GET", ["api", "settings"]) => Some(get()),
        ("PATCH", ["api", "settings"]) => Some(patch(ctx)),
        ("PATCH", ["api", "env"]) => Some(env_patch(ctx)),
        _ => None,
    }
}

/// `PATCH /api/env` — `api_env_patch`. Allowlist-gated (400 otherwise). Python
/// writes `.env`; the in-process app has no `.env` load, so we set the live process
/// env var (so the GET /api/settings overlay + the source clients see it this
/// session) and persist it through user settings.
/// ponytail: `set_var` mutates global env — fine for a rare settings PATCH, not a
/// hot path. Across-restart persistence is via user_settings.json, not `.env`.
fn env_patch(ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    struct Body {
        key: String,
        value: String,
    }
    let b: Body = ctx.parse_body()?;
    if !ALLOWED_ENV_KEYS.contains(&b.key.as_str()) {
        return Err(ApiError::new(
            400,
            format!("Key '{}' is not settable via this endpoint", b.key),
        ));
    }
    std::env::set_var(&b.key, &b.value);
    UserSettings::load()?.set(b.key, Value::String(b.value))?;
    Ok(json!({ "ok": true }))
}

/// `GET /api/settings` — `api_settings_get`. Settings first, then each env key
/// overlaid (present env keys win; missing ones are skipped, as in app.py).
fn get() -> Result<Value, ApiError> {
    let settings = UserSettings::load()?.all();
    let env: Vec<(&str, Option<String>)> = SETTINGS_ENV_KEYS
        .iter()
        .map(|&k| (k, std::env::var(k).ok()))
        .collect();
    Ok(Value::Object(overlay_env(settings, &env)))
}

/// `PATCH /api/settings` — `api_settings_patch`. Loops `set(k, v)` over the body.
fn patch(ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    struct Body {
        updates: Map<String, Value>,
    }
    let b: Body = ctx.parse_body()?;
    let mut settings = UserSettings::load()?;
    for (key, value) in b.updates {
        settings.set(key, value)?;
    }
    Ok(json!({ "ok": true }))
}

/// Overlay env values onto the settings map: Python `settings[key] = value` for
/// each present env var. Insert keeps an existing key's position and appends a
/// new one (preserve_order Map == Python dict), so the merge order matches app.py.
fn overlay_env(mut settings: Map<String, Value>, env: &[(&str, Option<String>)]) -> Map<String, Value> {
    for (key, value) in env {
        if let Some(v) = value {
            settings.insert((*key).to_string(), Value::String(v.clone()));
        }
    }
    settings
}

#[cfg(test)]
mod tests {
    // The GET arm calls UserSettings::load() (real settings file) and the test
    // rule forbids redirecting the data dir, so GET has no isolated test; the
    // overlay logic — the only nontrivial part — is pinned by the unit tests below.
    use super::*;

    #[test]
    fn overlay_appends_present_env_keys_in_order_after_settings() {
        let mut base = Map::new();
        base.insert("theme".into(), json!("dark"));
        let env = [("CROSSREF_MAILTO", Some("a@b.c".to_string())), ("OPENALEX_MAILTO", None)];
        let merged = overlay_env(base, &env);
        assert_eq!(
            serde_json::to_string(&Value::Object(merged)).unwrap(),
            r#"{"theme":"dark","CROSSREF_MAILTO":"a@b.c"}"#
        );
    }

    #[test]
    fn overlay_present_key_updates_in_place_keeping_position() {
        let mut base = Map::new();
        base.insert("CROSSREF_MAILTO".into(), json!("old"));
        base.insert("theme".into(), json!("dark"));
        let env = [("CROSSREF_MAILTO", Some("new".to_string())), ("OPENALEX_MAILTO", None)];
        let merged = overlay_env(base, &env);
        assert_eq!(
            serde_json::to_string(&Value::Object(merged)).unwrap(),
            r#"{"CROSSREF_MAILTO":"new","theme":"dark"}"#
        );
    }
}
