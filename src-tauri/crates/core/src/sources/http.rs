//! http — shared async client, host-allowlist + redirect guard, arxiv rate limit.
//! Port of the host/pacing logic in `sources/arxiv_source.py`,
//! `sources/arxiv_downloads.py` and `sources/fetch_paper_metadata.py`. Plan §5.4.

use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use reqwest::header::LOCATION;
use reqwest::Url;

use crate::error::{CoreError, Result};

/// arXiv host allowlist (exact-or-`.suffix`). Mirrors Python's pinned hosts.
pub const ARXIV_HOSTS: &[&str] = &["arxiv.org", "ar5iv.labs.arxiv.org", "export.arxiv.org"];
/// Polite mirror substituted into arXiv URLs, like `_substitute_domain(..., DOWNLOAD_DOMAIN)`.
const DOWNLOAD_DOMAIN: &str = "export.arxiv.org";
/// Cool-down after a 429 (`_RATELIMIT_WAIT` in fetch_paper_metadata.py).
const RATELIMIT_WAIT: Duration = Duration::from_secs(60);
/// Minimum spacing between successive arXiv requests. Mirrors the CONFIGURED
/// `arxiv.Client(delay_seconds=7.0)` (arxiv_source.py:68, fetch_paper_metadata.py:9),
/// NOT the library default — Plan §5.4 pins 7s to avoid an arXiv ban-risk regression.
const MIN_SPACING: Duration = Duration::from_secs(7);
/// Retries on a failed arXiv GET (arxiv.Client num_retries=1).
const NUM_RETRIES: usize = 1;
/// Redirect-follow ceiling for the guarded GET.
const MAX_REDIRECTS: usize = 10;

pub(crate) const USER_AGENT: &str =
    "linXiv/0.2 (+https://github.com/jakeuribe/linXiv; mailto:jake.uribe@gmail.com)";

/// Shared, sensibly-configured async client (UA + timeouts). Cheap to clone.
///
/// Redirects are disabled at the client level: `get_guarded` follows them by
/// hand so the host allowlist is re-checked on every hop.
pub fn client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .connect_timeout(Duration::from_secs(10))
                // Per-read, not total: a slow-but-steady large PDF stream must not be killed.
                .read_timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("reqwest client build")
        })
        .clone()
}

/// Allow a host iff it equals an entry or is a `.<entry>` subdomain of one.
pub fn assert_host_allowed(url: &str, allow: &[&str]) -> Result<()> {
    let parsed =
        Url::parse(url).map_err(|e| CoreError::BadRequest(format!("invalid url {url:?}: {e}")))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| CoreError::BadRequest(format!("url has no host: {url:?}")))?;
    let ok = allow
        .iter()
        .any(|a| host == *a || host.ends_with(&format!(".{a}")));
    if ok {
        Ok(())
    } else {
        Err(CoreError::BadRequest(format!(
            "host {host:?} is not in the allowlist {allow:?}"
        )))
    }
}

/// GET with the allowlist enforced on the initial URL and every redirect hop.
///
/// A 3xx to a disallowed host is rejected *before* the next request is sent, so
/// a hostile redirect never reaches the network. Non-3xx responses (including
/// 429) are returned to the caller as-is.
pub async fn get_guarded(url: &str, allow: &[&str]) -> Result<reqwest::Response> {
    get_guarded_with(url, allow, &[]).await
}

/// `get_guarded` plus per-request headers (e.g. OpenAlex's polite-pool
/// `User-Agent`). Headers are re-sent on every redirect hop; the host allowlist
/// is still enforced on each hop, so the extra headers never weaken the guard.
pub async fn get_guarded_with(
    url: &str,
    allow: &[&str],
    headers: &[(&str, &str)],
) -> Result<reqwest::Response> {
    let client = client();
    let mut current = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        assert_host_allowed(&current, allow)?;
        let mut req = client.get(&current);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| CoreError::Upstream(format!("GET {current:?} failed: {e}")))?;
        if resp.status().is_redirection() {
            let loc = resp
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    CoreError::Upstream(format!("redirect from {current:?} without Location"))
                })?;
            // Resolve relative Locations against the URL we just fetched.
            let next = Url::parse(&current)
                .and_then(|base| base.join(loc))
                .map_err(|e| CoreError::Upstream(format!("bad redirect target {loc:?}: {e}")))?;
            current = next.into();
            continue;
        }
        return Ok(resp);
    }
    Err(CoreError::Upstream(format!(
        "too many redirects (>{MAX_REDIRECTS}) starting at {url:?}"
    )))
}

/// Replace a URL's host, preserving scheme/path/query (`_substitute_domain`).
fn substitute_domain(url: &str, domain: &str) -> Result<String> {
    let mut parsed =
        Url::parse(url).map_err(|e| CoreError::BadRequest(format!("invalid url {url:?}: {e}")))?;
    parsed
        .set_host(Some(domain))
        .map_err(|e| CoreError::BadRequest(format!("cannot set host {domain:?}: {e}")))?;
    Ok(parsed.into())
}

/// Remaining cool-down if `.arxiv_ratelimit` under `data_dir` was written within
/// `RATELIMIT_WAIT` of `now`; `None` if no file, unparseable, or already elapsed.
/// Pure (clock injected) so the timing is unit-testable without sleeping.
fn cooldown_remaining(data_dir: &Path, now: DateTime<Utc>) -> Option<Duration> {
    let contents = std::fs::read_to_string(data_dir.join(".arxiv_ratelimit")).ok()?;
    let last = DateTime::parse_from_rfc3339(contents.trim())
        .ok()?
        .with_timezone(&Utc);
    let elapsed = now.signed_duration_since(last).to_std().ok()?;
    RATELIMIT_WAIT.checked_sub(elapsed)
}

/// Record "rate-limited now" so a later process honours the cool-down.
fn record_ratelimit(data_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(data_dir)
        .and_then(|_| std::fs::write(data_dir.join(".arxiv_ratelimit"), Utc::now().to_rfc3339()))
        .map_err(|e| CoreError::Internal(format!("write .arxiv_ratelimit: {e}")))
}

/// Block until at least `MIN_SPACING` has elapsed since the previous arXiv GET.
async fn enforce_spacing() {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let wait = {
        let guard = LAST.lock().unwrap();
        guard.and_then(|prev| MIN_SPACING.checked_sub(prev.elapsed()))
    };
    if let Some(w) = wait {
        tokio::time::sleep(w).await;
    }
    *LAST.lock().unwrap() = Some(Instant::now());
}

/// arXiv GET: honour the `.arxiv_ratelimit` cool-down file + inter-request
/// spacing under `data_dir`, substitute the polite `export.arxiv.org` mirror,
/// then `get_guarded` against the arXiv allowlist. Records the cool-down on 429.
pub async fn arxiv_get(url: &str, data_dir: &Path) -> Result<reqwest::Response> {
    if let Some(remaining) = cooldown_remaining(data_dir, Utc::now()) {
        tokio::time::sleep(remaining).await;
    }
    let target = substitute_domain(url, DOWNLOAD_DOMAIN)?;

    let mut last_err = None;
    for _ in 0..=NUM_RETRIES {
        enforce_spacing().await;
        match get_guarded(&target, ARXIV_HOSTS).await {
            Ok(resp) if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS => {
                record_ratelimit(data_dir)?;
                return Err(CoreError::Upstream(
                    "arXiv returned 429 — rate limited; retry in 60s".into(),
                ));
            }
            Ok(resp) => return Ok(resp),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| CoreError::Upstream("arXiv GET failed".into())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn host_allow_exact_and_subdomain() {
        assert!(assert_host_allowed("https://arxiv.org/abs/1", ARXIV_HOSTS).is_ok());
        assert!(assert_host_allowed("https://export.arxiv.org/x", ARXIV_HOSTS).is_ok());
        // ".suffix" subdomain of an allowed host.
        assert!(assert_host_allowed("https://foo.arxiv.org/x", ARXIV_HOSTS).is_ok());
    }

    #[test]
    fn host_allow_rejects_spoofs() {
        // Suffix-without-dot and lookalikes must NOT match (the classic allowlist bug).
        assert!(assert_host_allowed("https://notarxiv.org/x", ARXIV_HOSTS).is_err());
        assert!(assert_host_allowed("https://evilarxiv.org/x", ARXIV_HOSTS).is_err());
        // Allowed host as a left-label of an attacker domain.
        assert!(assert_host_allowed("https://arxiv.org.evil.com/x", ARXIV_HOSTS).is_err());
        assert!(assert_host_allowed("not a url", ARXIV_HOSTS).is_err());
    }

    #[test]
    fn substitute_domain_keeps_scheme_path_query() {
        let out = substitute_domain("https://arxiv.org/pdf/2204.12985v4?v=2", "export.arxiv.org")
            .unwrap();
        assert_eq!(out, "https://export.arxiv.org/pdf/2204.12985v4?v=2");
    }

    #[test]
    fn cooldown_recorded_then_elapses() {
        let dir = tempfile::tempdir().unwrap();
        // No file yet → no cool-down.
        assert!(cooldown_remaining(dir.path(), Utc::now()).is_none());

        record_ratelimit(dir.path()).unwrap();
        let now = Utc::now();
        // Just recorded → ~60s remaining (allow a little slack for test wall-time).
        let remaining = cooldown_remaining(dir.path(), now).expect("fresh cool-down present");
        assert!(
            remaining > Duration::from_secs(55) && remaining <= RATELIMIT_WAIT,
            "remaining was {remaining:?}"
        );
        // A clock 61s later → cool-down has elapsed.
        assert!(cooldown_remaining(dir.path(), now + chrono::Duration::seconds(61)).is_none());
    }

    #[tokio::test]
    async fn guarded_follows_same_host_redirect() {
        let server = MockServer::start().await;
        let final_url = format!("{}/final", server.uri());
        Mock::given(method("GET"))
            .and(path("/start"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", final_url.as_str()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/final"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;

        let resp = get_guarded(&format!("{}/start", server.uri()), &["127.0.0.1"])
            .await
            .expect("same-host redirect should be followed");
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await.unwrap(), "ok");
    }

    #[tokio::test]
    async fn guarded_rejects_offsite_redirect() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/start"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", "http://evil.example.com/x"),
            )
            .mount(&server)
            .await;

        let err = get_guarded(&format!("{}/start", server.uri()), &["127.0.0.1"])
            .await
            .expect_err("redirect to a disallowed host must be rejected");
        assert_eq!(err.http_status(), 400, "got {err}");
    }

    #[tokio::test]
    async fn guarded_returns_429_unfollowed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/limited"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;

        let resp = get_guarded(&format!("{}/limited", server.uri()), &["127.0.0.1"])
            .await
            .expect("429 is returned, not an error");
        assert_eq!(resp.status(), 429);
    }
}
