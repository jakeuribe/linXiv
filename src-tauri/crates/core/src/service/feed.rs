//! RSS feed pipeline — the one seam for the home feed (ADR-0010): async
//! network fetch (`fetch`), sync cache apply (`apply_fetch`), the filtered
//! read model (`read_page`), and the dismissal/rule mutations. Callers
//! (route today, CLI/MCP later) sequence fetch → apply → read without ever
//! touching `storage::queries::rss` or `sources::feed` directly; the
//! fetch/apply split exists so no DB lock is held across the await.

use std::path::Path;

use chrono::NaiveDateTime;
use rusqlite::Connection;
use serde_json::Value;

use crate::error::{CoreError, Result};
use crate::sources::feed as src_feed;
use crate::storage::queries::paper;
use crate::storage::queries::rss;

pub use crate::storage::queries::rss::{FilterAction, FilterField, FilterRule};

/// A fetched feed reduced to what the cache window persists.
pub struct FetchedFeed {
    pub title: String,
    pub entries: Vec<rss::CacheEntry>,
}

/// The filtered feed page: survivors of block/dismissal/rule filtering
/// (recorded as seen), plus which of them are already in the library.
/// `window_was_empty` reports the pre-filter DB window state, so a caller
/// holding a fetch error can distinguish "nothing cached at all" (surface the
/// error) from "everything filtered out" (serve the empty page).
pub struct FeedPage {
    pub entries: Vec<Value>,
    pub saved_arxiv_ids: Vec<String>,
    pub window_was_empty: bool,
}

/// `GET /api/feed` envelope; `entries` are untyped cached blobs, so the TS type stays hand-written.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FeedResponse {
    pub title: String,
    pub entries: Vec<Value>,
    pub saved_arxiv_ids: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
pub struct FeedRulesResponse {
    pub rules: Vec<FilterRule>,
}

#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
pub struct CreatedFeedRule {
    pub rule_id: i64,
}

/// Parse a feed entry's `published` string (RSS is RFC 822, Atom is RFC 3339).
/// `None` if neither parses -- caller then falls back to `FETCHED_AT`.
fn parse_published(s: &str) -> Option<NaiveDateTime> {
    chrono::DateTime::parse_from_rfc2822(s)
        .or_else(|_| chrono::DateTime::parse_from_rfc3339(s))
        .map(|dt| dt.naive_utc())
        .ok()
}

/// Build the DB row for a freshly-fetched entry. Dedup key is arxiv
/// `id+version` when present (so v2 doesn't overwrite v1), else link, else
/// title. `None` if the entry has none of those to key by.
fn to_cache_entry(entry: &src_feed::FeedEntry) -> Option<rss::CacheEntry> {
    let entry_json = serde_json::to_string(entry).ok()?;
    let source_id = entry.arxiv_id.as_deref().map(|id| format!("arxiv:{id}"));
    let dedup_key = match (&source_id, entry.version) {
        (Some(sid), Some(v)) => format!("{sid}v{v}"),
        _ if !entry.link.is_empty() => entry.link.clone(),
        _ if !entry.title.is_empty() => entry.title.clone(),
        _ => return None,
    };
    Some(rss::CacheEntry {
        dedup_key,
        source_id,
        entry_json,
        published_at: parse_published(&entry.published),
    })
}

/// `arxiv:{id}` source_id + parsed version for a cached entry, when it has an
/// arXiv id.
fn entry_identity(entry: &Value) -> (Option<String>, Option<i64>) {
    let source_id = entry
        .get("arxiv_id")
        .and_then(|v| v.as_str())
        .map(|id| format!("arxiv:{id}"));
    let version = entry.get("version").and_then(|v| v.as_i64());
    (source_id, version)
}

/// Async phase: fetch the feed over the network and reduce it to cache rows.
/// No DB access — callers apply the result with `apply_fetch` under the lock.
pub async fn fetch(url: &str, data_dir: &Path) -> Result<FetchedFeed> {
    eprintln!("[linxiv] feed: fetching {url}");
    let feed = src_feed::fetch_feed(url, data_dir).await?;
    eprintln!(
        "[linxiv] feed: fetched {url} ({} entries)",
        feed.entries.len()
    );
    let entries = feed.entries.iter().filter_map(to_cache_entry).collect();
    Ok(FetchedFeed {
        title: feed.title,
        entries,
    })
}

/// Sync phase: additively merge fetched rows into the per-URL window, then
/// prune rows that aged out of retention.
pub fn apply_fetch(
    conn: &mut Connection,
    url: &str,
    fresh: &[rss::CacheEntry],
    retention_days: i64,
) -> Result<()> {
    rss::merge_cache_entries(conn, url, fresh)?;
    rss::prune_cache_entries(conn, url, retention_days)?;
    Ok(())
}

/// Drop dismissed-paper bookkeeping older than the retention window.
pub fn prune_dismissed(conn: &Connection, retention_days: i64) -> Result<()> {
    rss::prune_dismissed(conn, retention_days)
}

/// The read model: the cached window with blocks, dismissals, and filter
/// rules applied, survivors recorded as seen, saved-to-library ids annotated.
pub fn read_page(conn: &mut Connection, url: &str, retention_days: i64) -> Result<FeedPage> {
    let mut entries = rss::load_cache_entries(conn, url, retention_days)?;
    let window_was_empty = entries.is_empty();
    let saved_arxiv_ids = annotate_and_filter(conn, &mut entries);
    Ok(FeedPage {
        entries,
        saved_arxiv_ids,
        window_was_empty,
    })
}

/// Drops dismissed/rule-hidden entries, records survivors as seen, and
/// returns which of them are already saved to the library. Failures are
/// logged and degrade to an unfiltered page rather than failing the request.
fn annotate_and_filter(conn: &mut Connection, entries: &mut Vec<Value>) -> Vec<String> {
    // One transaction for the whole read+write batch.
    let tx = match conn.transaction() {
        Ok(tx) => tx,
        Err(e) => {
            eprintln!("[linxiv] feed annotate_and_filter: failed to start transaction: {e}");
            return Vec::new();
        }
    };
    let blocked = rss::blocked_source_ids(&tx).unwrap_or_default();
    let dismissed_versions = rss::dismissed_versions(&tx).unwrap_or_default();
    let rules = rss::compile_rules(rss::list_rules(&tx).unwrap_or_default());

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
    // Truncate after filtering, not before, so hidden entries don't eat
    // into the 200 the client gets to see.
    entries.truncate(200);

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

    let source_ids: Vec<String> = entries
        .iter()
        .filter_map(|entry| entry.get("arxiv_id").and_then(|id| id.as_str()))
        .map(|arxiv_id| format!("arxiv:{arxiv_id}"))
        .collect();
    let stored: std::collections::HashSet<String> =
        match paper::existing_source_ids(&tx, &source_ids) {
            Ok(ids) => ids.into_iter().collect(),
            Err(e) => {
                eprintln!("[linxiv] feed saved-check failed: {e}");
                Default::default()
            }
        };
    let saved_arxiv_ids = entries
        .iter()
        .filter_map(|entry| entry.get("arxiv_id").and_then(|id| id.as_str()))
        .filter(|arxiv_id| stored.contains(&format!("arxiv:{arxiv_id}")))
        .map(str::to_string)
        .collect();

    if let Err(e) = tx.commit() {
        eprintln!("[linxiv] feed annotate_and_filter: commit failed: {e}");
    }
    saved_arxiv_ids
}

/// Hide an entry. `permanent` blocks the whole paper; otherwise dismisses
/// just this `version`.
pub fn dismiss(conn: &Connection, arxiv_id: &str, version: i64, permanent: bool) -> Result<()> {
    if arxiv_id.trim().is_empty() {
        return Err(CoreError::Validation("arxiv_id is required".into()));
    }
    rss::dismiss(conn, &format!("arxiv:{arxiv_id}"), version, permanent)
}

/// List auto-filter rules.
pub fn list_rules(conn: &Connection) -> Result<Vec<FilterRule>> {
    rss::list_rules(conn)
}

/// Create an auto-filter rule. Field/action validity is the type's job
/// (deserialization rejects unknown variants); only keywords needs checking.
pub fn create_rule(
    conn: &Connection,
    field: FilterField,
    keywords: &str,
    action: FilterAction,
) -> Result<i64> {
    if keywords.trim().is_empty() {
        return Err(CoreError::Validation("keywords is required".into()));
    }
    rss::create_rule(conn, field, keywords, action)
}

/// Remove an auto-filter rule; `NotFound` when no such rule exists.
pub fn delete_rule(conn: &Connection, rule_id: i64) -> Result<()> {
    if !rss::delete_rule(conn, rule_id)? {
        return Err(CoreError::NotFound("no such rule".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{init_db, open_in_memory};

    fn conn() -> Connection {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn
    }

    /// Wire-shape pin: a rules-list row exactly as `GET /api/feed/rules`
    /// serializes it.
    #[test]
    fn filter_rule_wire_shape() {
        assert_eq!(
            serde_json::to_string(&FilterRule {
                rule_id: 1,
                field: FilterField::Title,
                keywords: "llm".into(),
                action: FilterAction::Deny,
                enabled: true,
            })
            .unwrap(),
            r#"{"rule_id":1,"field":"TITLE","keywords":"llm","action":"DENY","enabled":true}"#
        );
    }

    /// Dismissed versions drop out of the page; survivors are recorded as
    /// seen; `window_was_empty` reflects the pre-filter window.
    #[test]
    fn read_page_filters_dismissed_and_reports_raw_window() {
        let mut c = conn();
        let url = "https://example.com/feed";
        apply_fetch(
            &mut c,
            url,
            &[
                rss::CacheEntry {
                    dedup_key: "arxiv:1111.00001v1".into(),
                    source_id: Some("arxiv:1111.00001".into()),
                    entry_json: r#"{"title":"Keep","arxiv_id":"1111.00001","version":1}"#.into(),
                    published_at: None,
                },
                rss::CacheEntry {
                    dedup_key: "arxiv:2222.00002v1".into(),
                    source_id: Some("arxiv:2222.00002".into()),
                    entry_json: r#"{"title":"Drop","arxiv_id":"2222.00002","version":1}"#.into(),
                    published_at: None,
                },
            ],
            30,
        )
        .unwrap();
        dismiss(&c, "2222.00002", 1, false).unwrap();

        let page = read_page(&mut c, url, 30).unwrap();
        assert!(!page.window_was_empty);
        let titles: Vec<&str> = page
            .entries
            .iter()
            .filter_map(|e| e.get("title").and_then(|t| t.as_str()))
            .collect();
        assert_eq!(titles, ["Keep"]);

        let empty = read_page(&mut c, "https://example.com/other", 30).unwrap();
        assert!(empty.window_was_empty);
        assert!(empty.entries.is_empty());
    }

    /// Saved-check pin: only entries whose paper is stored *and active* are
    /// annotated — trashed and never-saved papers are not, and duplicate
    /// arxiv_ids in the window are annotated per entry.
    #[test]
    fn read_page_annotates_saved_active_papers_only() {
        use crate::models::PaperMetadata;
        use crate::service::paper as svc_paper;

        let mut c = conn();
        let url = "https://example.com/feed";
        for sid in ["arxiv:1111.00001", "arxiv:2222.00002"] {
            let meta = PaperMetadata {
                source_id: sid.into(),
                version: 1,
                title: "T".into(),
                authors: vec!["Alice".into()],
                published: chrono::NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
                updated: None,
                summary: "s".into(),
                category: None,
                categories: None,
                doi: None,
                journal_ref: None,
                comment: None,
                url: None,
                tags: None,
                source: Some("arxiv".into()),
                author_orcids: None,
            };
            svc_paper::save_paper_metadata(&mut c, &meta, None).unwrap();
        }
        svc_paper::delete(
            &mut c,
            &svc_paper::PaperRef::source("arxiv:2222.00002".into()),
        )
        .unwrap();

        let entry = |id: &str, key: &str| rss::CacheEntry {
            dedup_key: key.into(),
            source_id: Some(format!("arxiv:{id}")),
            entry_json: format!(r#"{{"title":"T","arxiv_id":"{id}","version":1}}"#),
            published_at: None,
        };
        apply_fetch(
            &mut c,
            url,
            &[
                entry("1111.00001", "a"),
                entry("2222.00002", "b"), // trashed -> not saved
                entry("3333.00003", "c"), // never stored
                entry("1111.00001", "d"), // duplicate id, distinct dedup key
            ],
            30,
        )
        .unwrap();

        let page = read_page(&mut c, url, 30).unwrap();
        assert_eq!(page.entries.len(), 4);
        assert_eq!(page.saved_arxiv_ids, ["1111.00001", "1111.00001"]);
    }

    #[test]
    fn create_rule_requires_keywords() {
        let c = conn();
        assert!(matches!(
            create_rule(&c, FilterField::Title, "  ", FilterAction::Deny),
            Err(CoreError::Validation(_))
        ));
        let id = create_rule(&c, FilterField::Title, "llm", FilterAction::Deny).unwrap();
        delete_rule(&c, id).unwrap();
        assert!(matches!(delete_rule(&c, id), Err(CoreError::NotFound(_))));
    }

    /// Invalid field/action strings die at deserialization now, not in the
    /// service -- pin that "BODY"/"MAYBE" are rejected on the wire.
    #[test]
    fn invalid_field_action_fail_deserialization() {
        assert!(serde_json::from_str::<FilterField>(r#""BODY""#).is_err());
        assert!(serde_json::from_str::<FilterAction>(r#""MAYBE""#).is_err());
        assert_eq!(
            serde_json::from_str::<FilterField>(r#""TITLE""#).unwrap(),
            FilterField::Title
        );
        assert_eq!(
            serde_json::from_str::<FilterAction>(r#""ALLOW""#).unwrap(),
            FilterAction::Allow
        );
    }
}
