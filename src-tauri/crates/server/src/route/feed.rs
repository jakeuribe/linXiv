//! `GET /api/feed?url=…` — fetch the user's home RSS/Atom feed into a rolling
//! `RSS_CACHE_ENTRY` window, throttled per `CACHE_TTL`, dismissed/rule-filtered before response.
//! Thin handlers over `service::feed`; only the in-process fetch throttle
//! (GUI polling concern, useless to one-shot CLI runs) lives here.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};

use linxiv_core::config::UserSettings;
use linxiv_core::service::feed as svc_feed;
use linxiv_core::service::feed::{FilterAction, FilterField};

use crate::route::{ApiError, ReqCtx};
use crate::state::AppState;

const CACHE_TTL: Duration = Duration::from_secs(300);

// Throttle state: last-fetch time + channel title per URL (title isn't stored
// in the DB window, so a throttled request still needs it from here).
static LAST_FETCH: LazyLock<Mutex<HashMap<String, (Instant, String)>>> =
    LazyLock::new(Default::default);

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

async fn get_feed(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let url = ctx
        .q("url")
        .filter(|u| !u.trim().is_empty())
        .ok_or_else(|| ApiError::new(422, "url query parameter is required"))?;

    let last_fetch = LAST_FETCH
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(url)
        .cloned();
    let due = last_fetch
        .as_ref()
        .map(|(at, _)| at.elapsed() >= CACHE_TTL)
        .unwrap_or(true);
    let mut title = last_fetch.map(|(_, t)| t).unwrap_or_default();
    let retention_days = UserSettings::load()
        .map(|s| s.rss_cache_retention_days())
        .unwrap_or(30);

    let mut fetch_err = None;
    if due {
        match refresh(state, url, retention_days).await {
            Ok(t) => title = t,
            // No throttle entry on failure, so the next request retries instead
            // of waiting out the TTL. Fall through to serve the DB window.
            Err(e) => {
                eprintln!("[linxiv] feed: fetch failed for {url}: {}", e.detail);
                fetch_err = Some(e);
            }
        }
    }

    let page = state.with_conn(|conn| svc_feed::read_page(conn, url, retention_days))?;
    if page.window_was_empty {
        if let Some(e) = fetch_err {
            return Err(e);
        }
    }
    Ok(json!({
        "title": title,
        "entries": page.entries,
        "saved_arxiv_ids": page.saved_arxiv_ids,
    }))
}

/// One fetch-and-persist pass for `url`: prune, fetch, merge into the DB
/// window, record the throttle entry. Shared by `get_feed` and the headless
/// bin's poll loop. Returns the channel title.
pub async fn refresh(state: &AppState, url: &str, retention_days: i64) -> Result<String, ApiError> {
    // Cleanup before fetching, non-fatal (shouldn't fail the pass).
    if let Err(e) = state.with_conn(|conn| svc_feed::prune_dismissed(conn, retention_days)) {
        eprintln!("[linxiv] feed: prune_dismissed failed: {e}");
    }
    // data_dir carries the shared .arxiv_ratelimit file (same one arxiv_get uses).
    let fetched = svc_feed::fetch(url, &linxiv_core::config::data_dir()).await?;
    state.with_conn(|conn| svc_feed::apply_fetch(conn, url, &fetched.entries, retention_days))?;
    LAST_FETCH
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(url.to_string(), (Instant::now(), fetched.title.clone()));
    Ok(fetched.title)
}

/// `POST /api/feed/dismiss` — hide an entry. `permanent: true` blocks the
/// whole paper; otherwise dismisses just this `version` (the default).
fn dismiss(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    struct Body {
        arxiv_id: String,
        version: i64,
        #[serde(default)]
        permanent: bool,
    }
    let b: Body = ctx.parse_body()?;
    state.with_conn(|conn| svc_feed::dismiss(conn, &b.arxiv_id, b.version, b.permanent))?;
    Ok(json!({ "ok": true }))
}

/// `GET /api/feed/rules` — list auto-filter rules.
fn list_rules(state: &AppState) -> Result<Value, ApiError> {
    let rules = state.with_conn(|conn| svc_feed::list_rules(conn))?;
    Ok(json!({ "rules": rules }))
}

/// `POST /api/feed/rules` — create an auto-filter rule.
fn create_rule(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    struct Body {
        field: FilterField,
        keywords: String,
        #[serde(default = "default_action")]
        action: FilterAction,
    }
    fn default_action() -> FilterAction {
        FilterAction::Deny
    }
    let b: Body = ctx.parse_body()?;
    let rule_id =
        state.with_conn(|conn| svc_feed::create_rule(conn, b.field, &b.keywords, b.action))?;
    Ok(json!({ "rule_id": rule_id }))
}

/// `DELETE /api/feed/rules/{id}` — remove an auto-filter rule. 404 when unset.
fn delete_rule(state: &AppState, id: &str) -> Result<Value, ApiError> {
    let rule_id = crate::route::path_i64(id)?;
    state.with_conn(|conn| svc_feed::delete_rule(conn, rule_id))?;
    Ok(json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use crate::route::{route, ApiRequest};
    use crate::state::AppState;
    use linxiv_core::storage;
    use linxiv_core::storage::queries::rss;

    fn state() -> AppState {
        let conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        AppState::from_parts(conn, std::env::temp_dir(), std::env::temp_dir())
    }

    async fn get(path: &str) -> Result<serde_json::Value, crate::route::ApiError> {
        get_in(&state(), path).await
    }

    /// Like `get`, but against a caller-supplied state, so the DB cache can be
    /// pre-seeded and observed across the call.
    async fn get_in(
        state: &AppState,
        path: &str,
    ) -> Result<serde_json::Value, crate::route::ApiError> {
        route(
            state,
            ApiRequest {
                method: "GET".into(),
                path: path.into(),
                body: None,
            },
        )
        .await
    }

    async fn post(
        state: &AppState,
        path: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, crate::route::ApiError> {
        route(
            state,
            ApiRequest {
                method: "POST".into(),
                path: path.into(),
                body: Some(body),
            },
        )
        .await
    }

    /// Invalid field/action values die at body deserialization: 422 (same
    /// status the old service-side validation returned) with a detail naming
    /// the offending value and the valid variants.
    #[tokio::test]
    async fn create_rule_invalid_field_or_action_is_422_naming_the_value() {
        let st = state();
        let err = post(
            &st,
            "/api/feed/rules",
            serde_json::json!({ "field": "BODY", "keywords": "x" }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 422);
        assert!(err.detail.contains("BODY"), "got: {}", err.detail);
        assert!(err.detail.contains("TITLE"), "got: {}", err.detail);

        let err = post(
            &st,
            "/api/feed/rules",
            serde_json::json!({ "field": "TITLE", "keywords": "x", "action": "MAYBE" }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 422);
        assert!(err.detail.contains("MAYBE"), "got: {}", err.detail);
        assert!(err.detail.contains("DENY"), "got: {}", err.detail);
    }

    /// Omitted action still defaults to DENY with typed bodies.
    #[tokio::test]
    async fn create_rule_defaults_action_to_deny() {
        let st = state();
        let res = post(
            &st,
            "/api/feed/rules",
            serde_json::json!({ "field": "TITLE", "keywords": "llm" }),
        )
        .await
        .unwrap();
        assert!(res["rule_id"].as_i64().is_some());
        let rules = st
            .with_conn(|conn| linxiv_core::service::feed::list_rules(conn))
            .unwrap();
        assert_eq!(rules[0].action, super::FilterAction::Deny);
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

        // No LAST_FETCH.lock().unwrap().clear() here: LAST_FETCH is a process-wide
        // static and cargo test runs these #[tokio::test]s concurrently, so a blind
        // clear() races with sibling tests and can evict their entries mid-test.
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

        let mock_server = MockServer::start().await;
        // Distinct path: a reused ephemeral port + shared path would collide in LAST_FETCH.
        let feed_url = format!("{}/feed-saved-ids.json", mock_server.uri());

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

    /// An empty upstream fetch must not wipe previously-persisted entries.
    #[tokio::test]
    async fn empty_upstream_fetch_does_not_clobber_previously_cached_entries() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        let feed_url = format!("{}/weekend-feed.xml", mock_server.uri());
        let encoded_url = feed_url.replace(":", "%3A").replace("/", "%2F");

        let st = state();
        st.with_conn(|conn| {
            rss::merge_cache_entries(
                conn,
                &feed_url,
                &[rss::CacheEntry {
                    dedup_key: "arxiv:9999.00001v1".into(),
                    source_id: Some("arxiv:9999.00001".into()),
                    entry_json:
                        r#"{"title":"Yesterday's Paper","arxiv_id":"9999.00001","version":1}"#
                            .into(),
                    published_at: None,
                }],
            )
        })
        .unwrap();

        // Well-formed but empty upstream response -- the weekend case.
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"<?xml version="1.0"?><rss version="2.0"><channel><title>Empty</title></channel></rss>"#,
            ))
            .expect(1)
            .mount(&mock_server)
            .await;

        let result = get_in(&st, &format!("/api/feed?url={}", encoded_url))
            .await
            .unwrap();
        let titles: Vec<&str> = result["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e.get("title").and_then(|t| t.as_str()))
            .collect();
        assert!(
            titles.contains(&"Yesterday's Paper"),
            "empty upstream fetch must not wipe previously cached entries, got: {titles:?}"
        );

        mock_server.verify().await;
    }

    /// Two successive fetches accumulate rather than replace.
    #[tokio::test]
    async fn successive_fetches_accumulate_additively() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        let feed_url = format!("{}/growing-feed.xml", mock_server.uri());
        let encoded_url = feed_url.replace(":", "%3A").replace("/", "%2F");

        let st = state();
        st.with_conn(|conn| {
            rss::merge_cache_entries(
                conn,
                &feed_url,
                &[rss::CacheEntry {
                    dedup_key: "arxiv:1111.00001v1".into(),
                    source_id: Some("arxiv:1111.00001".into()),
                    entry_json: r#"{"title":"Day One Paper","arxiv_id":"1111.00001","version":1}"#
                        .into(),
                    published_at: None,
                }],
            )
        })
        .unwrap();

        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"<?xml version="1.0"?>
<rss version="2.0">
  <channel>
    <title>Growing Feed</title>
    <item>
      <title>Day Two Paper</title>
      <link>https://arxiv.org/abs/2222.00002v1</link>
    </item>
  </channel>
</rss>"#,
            ))
            .expect(1)
            .mount(&mock_server)
            .await;

        let result = get_in(&st, &format!("/api/feed?url={}", encoded_url))
            .await
            .unwrap();
        let titles: Vec<&str> = result["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e.get("title").and_then(|t| t.as_str()))
            .collect();
        assert!(titles.contains(&"Day One Paper"), "got: {titles:?}");
        assert!(titles.contains(&"Day Two Paper"), "got: {titles:?}");

        mock_server.verify().await;
    }

    /// The headless poll path: `refresh` alone fetches and persists entries
    /// into the DB window (no `get_feed` involved).
    #[tokio::test]
    async fn refresh_persists_entries_for_the_poll_loop() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        let feed_url = format!("{}/poll-feed.xml", mock_server.uri());

        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"<?xml version="1.0"?>
<rss version="2.0">
  <channel>
    <title>Poll Feed</title>
    <item>
      <title>Polled Paper</title>
      <link>https://arxiv.org/abs/3333.00003v1</link>
    </item>
  </channel>
</rss>"#,
            ))
            .expect(1)
            .mount(&mock_server)
            .await;

        let st = state();
        let title = super::refresh(&st, &feed_url, 30).await.unwrap();
        assert_eq!(title, "Poll Feed");
        let page = st
            .with_conn(|conn| linxiv_core::service::feed::read_page(conn, &feed_url, 30))
            .unwrap();
        let titles: Vec<&str> = page
            .entries
            .iter()
            .filter_map(|e| e.get("title").and_then(|t| t.as_str()))
            .collect();
        assert!(titles.contains(&"Polled Paper"), "got: {titles:?}");

        mock_server.verify().await;
    }
}
