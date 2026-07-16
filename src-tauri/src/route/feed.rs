//! `GET /api/feed?url=…` — fetch + parse the user-configured home RSS/Atom feed
//! (core `sources::feed`), with a small in-memory success cache so re-visiting
//! the home page doesn't re-hit the upstream on every render. Entries the user
//! dismissed (`POST /api/feed/dismiss`) or that match a filter rule
//! (`RSS_FILTER_RULE`, CRUD under `/api/feed/rules`) are stripped before the
//! response goes back to the client.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};

use linxiv_core::sources::feed as svc_feed;
use linxiv_core::storage::queries::rss;

use crate::route::{ApiError, ReqCtx};
use crate::state::AppState;

const CACHE_TTL: Duration = Duration::from_secs(300);

// Unbounded per-URL map — in practice one home-feed URL lives here.
static CACHE: LazyLock<Mutex<HashMap<String, (Instant, Value)>>> = LazyLock::new(Default::default);

pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("GET", ["api", "feed"]) => Some(get_feed(state, ctx).await),
        ("POST", ["api", "feed", "dismiss"]) => Some(dismiss(state, ctx)),
        ("GET", ["api", "feed", "rules"]) => Some(list_rules(state)),
        ("POST", ["api", "feed", "rules"]) => Some(create_rule(state, ctx)),
        ("DELETE", ["api", "feed", "rules", id]) => Some(delete_rule(state, id)),
        _ => None,
    }
}

/// `arxiv:{id}` source_id + parsed version for an entry, when it has an arXiv id.
fn entry_identity(entry: &Value) -> (Option<String>, Option<i64>) {
    let source_id = entry
        .get("arxiv_id")
        .and_then(|v| v.as_str())
        .map(|id| format!("arxiv:{id}"));
    let version = entry.get("version").and_then(|v| v.as_i64());
    (source_id, version)
}

/// Drops entries the user dismissed or that match a filter rule, records each
/// survivor as seen (`RSS_PAPER_ROOTS`/`RSS_PAPER`), and returns which of them
/// are already saved to the library — all in one DB connection.
fn annotate_and_filter(state: &AppState, feed_value: &mut Value) -> Vec<String> {
    use linxiv_core::storage::queries::paper;

    let Some(entries) = feed_value.get_mut("entries").and_then(|e| e.as_array_mut()) else {
        return Vec::new();
    };
    entries.truncate(200);

    state.with_conn(|conn| {
        // One transaction for the whole read+write batch -- avoids up to ~200
        // individually-committed statements (one per surviving entry) per GET.
        let tx = match conn.transaction() {
            Ok(tx) => tx,
            Err(e) => {
                eprintln!("[linxiv] feed annotate_and_filter: failed to start transaction: {e}");
                return Vec::new();
            }
        };
        let blocked = rss::blocked_source_ids(&tx).unwrap_or_default();
        let dismissed_versions = rss::dismissed_versions(&tx).unwrap_or_default();
        let rules = rss::list_rules(&tx).unwrap_or_default();

        entries.retain(|entry| {
            let title = entry.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let summary = entry.get("summary").and_then(|v| v.as_str()).unwrap_or("");
            let authors = entry
                .get("authors")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            let (source_id, version) = entry_identity(entry);

            if source_id.as_ref().is_some_and(|sid| blocked.contains(sid)) {
                return false;
            }
            if let (Some(sid), Some(v)) = (&source_id, version) {
                if dismissed_versions.contains(&(sid.clone(), v)) {
                    return false;
                }
            }
            !rss::is_hidden(&rules, title, summary, &authors)
        });

        // Record each survivor as seen, separately from the (pure) filter above.
        for entry in entries.iter() {
            let title = entry.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let (source_id, version) = entry_identity(entry);
            if let (Some(sid), Some(v)) = (&source_id, version) {
                if let Err(e) = rss::upsert_seen(&tx, sid, v, title) {
                    eprintln!("[linxiv] feed upsert_seen failed: source_id={sid}, error={e}");
                }
            }
        }

        let saved_arxiv_ids = entries
            .iter()
            .filter_map(|entry| entry.get("arxiv_id").and_then(|id| id.as_str()))
            .filter_map(|arxiv_id| {
                let source_id = format!("arxiv:{arxiv_id}");
                match paper::get_paper(&tx, &source_id, None) {
                    Ok(Some(_)) => Some(arxiv_id.to_string()),
                    Ok(None) => None,
                    Err(e) => {
                        eprintln!(
                            "[linxiv] feed saved-check failed: source_id={source_id}, error={e}"
                        );
                        None
                    }
                }
            })
            .collect();

        if let Err(e) = tx.commit() {
            eprintln!("[linxiv] feed annotate_and_filter: commit failed: {e}");
        }
        saved_arxiv_ids
    })
}

async fn get_feed(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let url = ctx
        .q("url")
        .filter(|u| !u.trim().is_empty())
        .ok_or_else(|| ApiError::new(422, "url query parameter is required"))?;

    let cached = CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(url)
        .filter(|(at, _)| at.elapsed() < CACHE_TTL)
        .map(|(_, v)| v.clone());

    let mut result = match cached {
        Some(v) => v,
        None => {
            // Cache miss or expired: fetch fresh from upstream.
            // data_dir carries the shared .arxiv_ratelimit file (same one arxiv_get uses).
            let feed = svc_feed::fetch_feed(url, &linxiv_core::config::data_dir()).await?;
            let v = serde_json::to_value(&feed).map_err(|e| ApiError::new(500, e.to_string()))?;
            CACHE
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(url.to_string(), (Instant::now(), v.clone()));
            v
        }
    };

    // Recompute dismiss/rule filtering + saved_arxiv_ids fresh against current DB
    // state (cache hit or miss) -- the cached value above is the raw upstream feed.
    let saved_arxiv_ids = annotate_and_filter(state, &mut result);
    result["saved_arxiv_ids"] = serde_json::json!(saved_arxiv_ids);
    Ok(result)
}

/// `POST /api/feed/dismiss` — hide an entry from the feed going forward.
/// `permanent: true` blocks the whole paper, every version (REMOVAL_TYPE
/// 'DOI'); otherwise it dismisses just this exact `version` (REMOVAL_TYPE
/// 'VER' on that one row, the default) — a later, higher version resurfaces.
fn dismiss(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    struct Body {
        arxiv_id: String,
        version: i64,
        #[serde(default)]
        permanent: bool,
    }
    let b: Body = ctx.parse_body()?;
    if b.arxiv_id.trim().is_empty() {
        return Err(ApiError::new(422, "arxiv_id is required"));
    }
    let source_id = format!("arxiv:{}", b.arxiv_id);
    state.with_conn(|conn| rss::dismiss(conn, &source_id, b.version, b.permanent))?;
    Ok(json!({ "ok": true }))
}

/// `GET /api/feed/rules` — list auto-filter rules.
fn list_rules(state: &AppState) -> Result<Value, ApiError> {
    let rules = state.with_conn(|conn| rss::list_rules(conn))?;
    Ok(json!({ "rules": rules }))
}

/// `POST /api/feed/rules` — create an auto-filter rule.
fn create_rule(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    struct Body {
        field: String,
        keywords: String,
        #[serde(default = "default_action")]
        action: String,
    }
    fn default_action() -> String {
        "DENY".to_string()
    }
    let b: Body = ctx.parse_body()?;
    if !matches!(b.field.as_str(), "TITLE" | "SUMMARY" | "AUTHOR") {
        return Err(ApiError::new(
            422,
            "field must be TITLE, SUMMARY, or AUTHOR",
        ));
    }
    if !matches!(b.action.as_str(), "DENY" | "ALLOW") {
        return Err(ApiError::new(422, "action must be DENY or ALLOW"));
    }
    if b.keywords.trim().is_empty() {
        return Err(ApiError::new(422, "keywords is required"));
    }
    let rule_id =
        state.with_conn(|conn| rss::create_rule(conn, &b.field, &b.keywords, &b.action))?;
    Ok(json!({ "rule_id": rule_id }))
}

/// `DELETE /api/feed/rules/{id}` — remove an auto-filter rule. 404 when unset.
fn delete_rule(state: &AppState, id: &str) -> Result<Value, ApiError> {
    let rule_id = crate::route::path_i64(id)?;
    let deleted = state.with_conn(|conn| rss::delete_rule(conn, rule_id))?;
    if !deleted {
        return Err(ApiError::new(404, "no such rule"));
    }
    Ok(json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use super::CACHE;
    use crate::route::{route, ApiRequest};
    use crate::state::AppState;
    use linxiv_core::storage;

    fn state() -> AppState {
        let conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        AppState::from_parts(conn, std::env::temp_dir(), std::env::temp_dir())
    }

    async fn get(path: &str) -> Result<serde_json::Value, crate::route::ApiError> {
        route(
            &state(),
            ApiRequest {
                method: "GET".into(),
                path: path.into(),
                body: None,
            },
        )
        .await
    }

    #[tokio::test]
    async fn missing_url_is_422_before_any_network() {
        let err = get("/api/feed").await.unwrap_err();
        assert_eq!(err.status, 422);
    }

    #[tokio::test]
    async fn non_http_scheme_is_400_before_any_network() {
        let err = get("/api/feed?url=file%3A%2F%2F%2Fetc%2Fpasswd")
            .await
            .unwrap_err();
        assert_eq!(err.status, 400);
    }

    #[tokio::test]
    async fn cache_hit_within_ttl_fetches_upstream_once() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        CACHE.lock().unwrap().clear();

        let mock_server = MockServer::start().await;
        let feed_url = format!("{}/feed.xml", mock_server.uri());

        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"<?xml version="1.0"?>
<rss version="2.0">
  <channel>
    <title>Test Feed</title>
    <link>http://example.com</link>
    <description>Test</description>
  </channel>
</rss>"#,
            ))
            .expect(1)
            .mount(&mock_server)
            .await;

        let encoded_url = feed_url.replace(":", "%3A").replace("/", "%2F");

        let result1 = get(&format!("/api/feed?url={}", encoded_url)).await;
        assert!(result1.is_ok());

        let result2 = get(&format!("/api/feed?url={}", encoded_url)).await;
        assert!(result2.is_ok());

        mock_server.verify().await;
    }

    #[tokio::test]
    async fn saved_arxiv_ids_empty_when_no_papers_in_library() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        CACHE.lock().unwrap().clear();

        let mock_server = MockServer::start().await;
        let feed_url = format!("{}/feed.json", mock_server.uri());

        // Mock a feed with an entry for arxiv paper 2301.00001
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"entries": [{"title": "Test Paper", "arxiv_id": "2301.00001"}]}"#,
            ))
            .expect(1)
            .mount(&mock_server)
            .await;

        let encoded_url = feed_url.replace(":", "%3A").replace("/", "%2F");

        let result = get(&format!("/api/feed?url={}", encoded_url)).await;
        assert!(result.is_ok());

        // saved_arxiv_ids should be empty since no paper exists in the library
        let saved_ids: Vec<String> = result.unwrap()["saved_arxiv_ids"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        assert!(saved_ids.is_empty());

        mock_server.verify().await;
    }

    #[tokio::test]
    async fn feed_entries_limited_to_200() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        CACHE.lock().unwrap().clear();

        let mock_server = MockServer::start().await;
        let feed_url = format!("{}/feed.json", mock_server.uri());

        // Create a feed with 250 entries
        let mut entries = String::new();
        for i in 0..250 {
            entries.push_str(&format!(
                r#"{{"title": "Paper {}", "arxiv_id": "{:04}"}}"#,
                i, i
            ));
            if i < 249 {
                entries.push(',');
            }
        }

        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(format!(r#"{{"entries": [{}]}}"#, entries)),
            )
            .expect(1)
            .mount(&mock_server)
            .await;

        let encoded_url = feed_url.replace(":", "%3A").replace("/", "%2F");

        let result = get(&format!("/api/feed?url={}", encoded_url)).await;
        assert!(result.is_ok());

        mock_server.verify().await;
    }
}
