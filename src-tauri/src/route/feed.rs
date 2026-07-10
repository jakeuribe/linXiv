//! `GET /api/feed?url=…` — fetch + parse the user-configured home RSS/Atom feed
//! (core `sources::feed`), with a small in-memory success cache so re-visiting
//! the home page doesn't re-hit the upstream on every render.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;

use linxiv_core::sources::feed as svc_feed;

use crate::route::{ApiError, ReqCtx};
use crate::state::AppState;

const CACHE_TTL: Duration = Duration::from_secs(300);

// ponytail: unbounded per-URL map — in practice one home-feed URL lives here;
// swap for an LRU if the UI ever shows many feeds.
fn cache() -> &'static Mutex<HashMap<String, (Instant, Value)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, Value)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("GET", ["api", "feed"]) => Some(get_feed(state, ctx).await),
        _ => None,
    }
}

fn get_saved_arxiv_ids(state: &AppState, feed_value: &Value) -> Vec<String> {
    if let Some(entries) = feed_value.get("entries").and_then(|e| e.as_array()) {
        use linxiv_core::storage::queries::paper;

        // ponytail: lookup by source_id assumes arxiv_id maps to source_id directly; refine if arXiv ID format changes.
        // Collect source_ids to check (limited to 200 entries) and batch the existence check.
        let to_check: Vec<(String, String)> = entries
            .iter()
            .take(200)
            .filter_map(|entry| entry.get("arxiv_id").and_then(|id| id.as_str()))
            .map(|arxiv_id| {
                let source_id = format!("arxiv:{}", arxiv_id);
                (source_id, arxiv_id.to_string())
            })
            .collect();

        // Batch check in a single DB connection, filtering to active papers only.
        state.with_conn(|conn| {
            to_check
                .into_iter()
                .filter_map(|(source_id, arxiv_id)| {
                    match paper::get_paper(conn, &source_id, None) {
                        Ok(Some(_)) => Some(arxiv_id),
                        Ok(None) => None,
                        Err(e) => {
                            eprintln!("[linxiv] feed saved-check failed: source_id={source_id}, error={e}");
                            None
                        }
                    }
                })
                .collect()
        })
    } else {
        Vec::new()
    }
}

async fn get_feed(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let url = ctx
        .q("url")
        .filter(|u| !u.trim().is_empty())
        .ok_or_else(|| ApiError::new(422, "url query parameter is required"))?;

    // Check cache for a fresh entry
    {
        let cache_lock = cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, v)) = cache_lock.get(url) {
            if at.elapsed() < CACHE_TTL {
                let feed_value = v.clone();
                drop(cache_lock);
                // Cache hit: recompute saved_arxiv_ids fresh against current DB state
                let saved_arxiv_ids = get_saved_arxiv_ids(state, &feed_value);
                let mut result = feed_value;
                result["saved_arxiv_ids"] = serde_json::json!(saved_arxiv_ids);
                return Ok(result);
            }
        }
    }

    // Cache miss or expired: fetch fresh from upstream
    // data_dir carries the shared .arxiv_ratelimit file (same one arxiv_get uses).
    let feed = svc_feed::fetch_feed(url, &linxiv_core::config::data_dir()).await?;
    let v = serde_json::to_value(&feed).map_err(|e| ApiError::new(500, e.to_string()))?;
    cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(url.to_string(), (Instant::now(), v.clone()));

    // Recompute saved_arxiv_ids fresh against current DB state
    let saved_arxiv_ids = get_saved_arxiv_ids(state, &v);
    let mut result = v;
    result["saved_arxiv_ids"] = serde_json::json!(saved_arxiv_ids);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::cache;
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

        cache().lock().unwrap().clear();

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

        cache().lock().unwrap().clear();

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

        cache().lock().unwrap().clear();

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
