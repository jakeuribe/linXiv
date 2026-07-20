//! RSS_PAPER_ROOTS / RSS_PAPER / RSS_FILTER_RULE queries — persisted state for the
//! home RSS feed: which entries have been seen and dismissed, and the keyword
//! rules that auto-hide entries before they reach the client.

use std::collections::HashSet;

use chrono::NaiveDateTime;
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::Value;

use crate::error::Result;

/// Record that a feed entry was seen (idempotent; upserts the root + version row).
pub fn upsert_seen(conn: &Connection, source_id: &str, version: i64, title: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO RSS_PAPER_ROOTS (SOURCE_ID) VALUES (?1)",
        [source_id],
    )?;
    let source_fk: i64 = conn.query_row(
        "SELECT SOURCE_FK FROM RSS_PAPER_ROOTS WHERE SOURCE_ID = ?1",
        [source_id],
        |r| r.get(0),
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO RSS_PAPER (SOURCE_ID, VERSION, TITLE, SOURCE_FK)
         VALUES (?1, ?2, ?3, ?4)",
        params![source_id, version, title, source_fk],
    )?;
    Ok(())
}

/// Hide a feed entry going forward. `permanent` blocks the whole paper, every
/// version, forever (`RSS_PAPER_ROOTS.REMOVAL_TYPE = 'DOI'`); otherwise it
/// dismisses just this exact version (`RSS_PAPER.REMOVAL_TYPE = 'VER'` on the
/// `(source_id, version)` row) — a later, higher version is a new row and
/// resurfaces undismissed.
pub fn dismiss(conn: &Connection, source_id: &str, version: i64, permanent: bool) -> Result<()> {
    if permanent {
        conn.execute(
            "INSERT INTO RSS_PAPER_ROOTS (SOURCE_ID, REMOVAL_TYPE, REMOVED_AT)
             VALUES (?1, 'DOI', datetime('now'))
             ON CONFLICT(SOURCE_ID) DO UPDATE SET
               REMOVAL_TYPE = 'DOI', REMOVED_AT = excluded.REMOVED_AT",
            [source_id],
        )?;
        return Ok(());
    }
    conn.execute(
        "INSERT OR IGNORE INTO RSS_PAPER_ROOTS (SOURCE_ID) VALUES (?1)",
        [source_id],
    )?;
    let source_fk: i64 = conn.query_row(
        "SELECT SOURCE_FK FROM RSS_PAPER_ROOTS WHERE SOURCE_ID = ?1",
        [source_id],
        |r| r.get(0),
    )?;
    conn.execute(
        "INSERT INTO RSS_PAPER (SOURCE_ID, VERSION, TITLE, SOURCE_FK, REMOVAL_TYPE, REMOVED_AT)
         VALUES (?1, ?2, '', ?3, 'VER', datetime('now'))
         ON CONFLICT(SOURCE_ID, VERSION) DO UPDATE SET
           REMOVAL_TYPE = 'VER', REMOVED_AT = excluded.REMOVED_AT",
        params![source_id, version, source_fk],
    )?;
    Ok(())
}

/// Every SOURCE_ID dismissed permanently (`RSS_PAPER_ROOTS.REMOVAL_TYPE = 'DOI'`).
pub fn blocked_source_ids(conn: &Connection) -> Result<HashSet<String>> {
    let mut stmt =
        conn.prepare("SELECT SOURCE_ID FROM RSS_PAPER_ROOTS WHERE REMOVAL_TYPE = 'DOI'")?;
    let rows = stmt.query_map([], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Every `(SOURCE_ID, VERSION)` pair dismissed at the version level
/// (`RSS_PAPER.REMOVAL_TYPE = 'VER'`).
pub fn dismissed_versions(conn: &Connection) -> Result<HashSet<(String, i64)>> {
    let mut stmt =
        conn.prepare("SELECT SOURCE_ID, VERSION FROM RSS_PAPER WHERE REMOVAL_TYPE = 'VER'")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// A freshly-fetched feed entry ready to persist into `RSS_CACHE_ENTRY`.
/// `dedup_key` is arxiv `source_id+version` when available, else the entry's
/// link (falls back to title) -- see `feed.rs::to_cache_entry`. `source_id` is
/// carried separately (without version), stored alongside the entry purely
/// for debugging/future use -- durable dismissal is checked directly against
/// `RSS_PAPER_ROOTS` in `annotate_and_filter`, not via this column.
pub struct CacheEntry {
    pub dedup_key: String,
    pub source_id: Option<String>,
    pub entry_json: String,
    pub published_at: Option<NaiveDateTime>,
}

/// Additively merge freshly-fetched entries into the persisted per-URL cache:
/// insert only entries whose dedup key isn't already stored for this URL, never
/// overwrite an existing row (`INSERT OR IGNORE` against the `(FEED_URL,
/// DEDUP_KEY)` unique index). This is what keeps an empty/short upstream fetch
/// (e.g. arXiv publishing nothing over a weekend) from clobbering what's
/// already cached -- it just merges in zero new rows. One transaction for the
/// whole batch, same reasoning as `annotate_and_filter`'s: avoids one commit
/// per entry on a fetch that can bring in hundreds.
pub fn merge_cache_entries(
    conn: &mut Connection,
    feed_url: &str,
    entries: &[CacheEntry],
) -> Result<()> {
    let tx = conn.transaction()?;
    for e in entries {
        tx.execute(
            "INSERT OR IGNORE INTO RSS_CACHE_ENTRY
                 (FEED_URL, DEDUP_KEY, SOURCE_ID, ENTRY_JSON, PUBLISHED_AT)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                feed_url,
                e.dedup_key,
                e.source_id,
                e.entry_json,
                // Space-separated to match the format `datetime('now')` writes into
                // FETCHED_AT -- COALESCE(PUBLISHED_AT, FETCHED_AT) is compared/ordered
                // as a raw SQL string below, and 'T' > ' ' would make same-day 'T'-form
                // timestamps sort as newer regardless of actual time.
                e.published_at
                    .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string()),
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Cap on rows returned per feed GET -- well above the 200 `annotate_and_filter`
/// truncates to, so dismissed/rule-filtered entries still leave enough headroom.
const MAX_LOADED_ENTRIES: i64 = 500;

/// Cached entries for `feed_url` within the retention window, newest-first by
/// published date (falling back to fetch time when the entry's own date didn't
/// parse). The window itself is measured by `FETCHED_AT` (when we first cached
/// the row), NOT `PUBLISHED_AT` -- an arXiv entry's `published` is its original
/// submission date, which for a just-updated v2/v3 of an old paper can be years
/// in the past even though we only just fetched it; keying the cutoff on that
/// would drop it from the window on the very fetch that added it. Malformed
/// stored JSON (should not happen; we wrote it) is logged and skipped rather
/// than failing the whole feed response. Durably-dismissed entries
/// (`RSS_PAPER_ROOTS.REMOVAL_TYPE = 'DOI'`) age out of this window like any
/// other row -- the dismissal itself lives in `RSS_PAPER_ROOTS`, which is what
/// `annotate_and_filter` actually checks, so nothing depends on keeping their
/// cache row around. Sorting newest-published-first while capping by fetch
/// recency is a deliberate tradeoff: if more than `MAX_LOADED_ENTRIES` rows are
/// within the window, a just-fetched update of an old paper can sort past the
/// cap despite being the newest fetch -- acceptable at the default retention
/// window/feed volume, revisit if `MAX_LOADED_ENTRIES` needs to grow.
pub fn load_cache_entries(
    conn: &Connection,
    feed_url: &str,
    retention_days: i64,
) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT ENTRY_FK, ENTRY_JSON
         FROM RSS_CACHE_ENTRY
         WHERE FEED_URL = ?1
           AND FETCHED_AT >= datetime('now', '-' || ?2 || ' days')
         ORDER BY COALESCE(PUBLISHED_AT, FETCHED_AT) DESC, ENTRY_FK DESC
         LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![feed_url, retention_days, MAX_LOADED_ENTRIES], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?;
    Ok(rows
        .collect::<rusqlite::Result<Vec<(i64, String)>>>()?
        .iter()
        .filter_map(|(entry_fk, s)| match serde_json::from_str(s) {
            Ok(v) => Some(v),
            Err(e) => {
                eprintln!("[linxiv] feed: malformed cached entry {entry_fk}: {e}");
                None
            }
        })
        .collect())
}

/// Delete rows outside the retention window (measured by `FETCHED_AT` -- see
/// `load_cache_entries` for why not `PUBLISHED_AT`).
pub fn prune_cache_entries(
    conn: &Connection,
    feed_url: &str,
    retention_days: i64,
) -> Result<usize> {
    Ok(conn.execute(
        "DELETE FROM RSS_CACHE_ENTRY
         WHERE FEED_URL = ?1
           AND FETCHED_AT < datetime('now', '-' || ?2 || ' days')",
        params![feed_url, retention_days],
    )?)
}

#[derive(Debug, Clone, Serialize)]
pub struct FilterRule {
    pub rule_id: i64,
    pub field: String,
    pub keywords: String,
    pub action: String,
    pub enabled: bool,
}

fn row_to_rule(r: &rusqlite::Row) -> rusqlite::Result<FilterRule> {
    Ok(FilterRule {
        rule_id: r.get(0)?,
        field: r.get(1)?,
        keywords: r.get(2)?,
        action: r.get(3)?,
        enabled: r.get::<_, i64>(4)? != 0,
    })
}

pub fn list_rules(conn: &Connection) -> Result<Vec<FilterRule>> {
    let mut stmt = conn.prepare(
        "SELECT RULE_ID, FIELD, KEYWORDS, ACTION, ENABLED FROM RSS_FILTER_RULE ORDER BY RULE_ID",
    )?;
    let rows = stmt.query_map([], row_to_rule)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn create_rule(conn: &Connection, field: &str, keywords: &str, action: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO RSS_FILTER_RULE (FIELD, KEYWORDS, ACTION) VALUES (?1, ?2, ?3)",
        params![field, keywords, action],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Returns false when no rule with that id existed.
pub fn delete_rule(conn: &Connection, rule_id: i64) -> Result<bool> {
    Ok(conn.execute("DELETE FROM RSS_FILTER_RULE WHERE RULE_ID = ?1", [rule_id])? > 0)
}

fn field_value<'a>(field: &str, title: &'a str, summary: &'a str, authors: &'a str) -> &'a str {
    match field {
        "TITLE" => title,
        "SUMMARY" => summary,
        "AUTHOR" => authors,
        _ => "",
    }
}

/// True if a feed entry should be hidden: some enabled DENY rule's keywords
/// (comma-separated, ALL must match -- AND, case-insensitive substring) match,
/// and no enabled ALLOW rule's keywords also match (ALLOW is the override).
pub fn is_hidden(rules: &[FilterRule], title: &str, summary: &str, authors: &str) -> bool {
    let rule_matches = |r: &FilterRule| -> bool {
        if !r.enabled {
            return false;
        }
        let keywords: Vec<String> = r
            .keywords
            .split(',')
            .map(|k| k.trim().to_lowercase())
            .filter(|k| !k.is_empty())
            .collect();
        // Separators-only keywords (e.g. ",") would make `.all()` vacuously
        // true and match every entry -- require at least one real keyword.
        if keywords.is_empty() {
            return false;
        }
        let field = field_value(&r.field, title, summary, authors).to_lowercase();
        keywords.iter().all(|kw| field.contains(kw))
    };
    let denied = rules
        .iter()
        .filter(|r| r.action == "DENY")
        .any(rule_matches);
    denied
        && !rules
            .iter()
            .filter(|r| r.action == "ALLOW")
            .any(rule_matches)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage;

    fn conn() -> Connection {
        let c = storage::open_in_memory().unwrap();
        storage::init_db(&c).unwrap();
        c
    }

    #[test]
    fn seen_then_dismissed_shows_up_in_dismissed_versions() {
        let c = conn();
        upsert_seen(&c, "arxiv:2401.00001", 1, "A Paper").unwrap();
        assert!(dismissed_versions(&c).unwrap().is_empty());
        assert!(blocked_source_ids(&c).unwrap().is_empty());

        dismiss(&c, "arxiv:2401.00001", 1, false).unwrap();
        let dismissed = dismissed_versions(&c).unwrap();
        assert!(dismissed.contains(&("arxiv:2401.00001".to_string(), 1)));
        assert!(blocked_source_ids(&c).unwrap().is_empty());
    }

    #[test]
    fn dismiss_without_prior_seen_still_hides() {
        let c = conn();
        dismiss(&c, "arxiv:2401.00002", 1, true).unwrap();
        assert!(blocked_source_ids(&c).unwrap().contains("arxiv:2401.00002"));
    }

    #[test]
    fn dismissing_one_version_does_not_hide_a_later_version() {
        let c = conn();
        upsert_seen(&c, "arxiv:2401.00003", 1, "v1 title").unwrap();
        dismiss(&c, "arxiv:2401.00003", 1, false).unwrap();
        assert!(dismissed_versions(&c)
            .unwrap()
            .contains(&("arxiv:2401.00003".to_string(), 1)));

        // v2 arrives: a distinct row, defaults to 'NOT', not in dismissed_versions.
        upsert_seen(&c, "arxiv:2401.00003", 2, "v2 title").unwrap();
        let dismissed = dismissed_versions(&c).unwrap();
        assert!(dismissed.contains(&("arxiv:2401.00003".to_string(), 1)));
        assert!(!dismissed.contains(&("arxiv:2401.00003".to_string(), 2)));
    }

    #[test]
    fn rule_crud_round_trips() {
        let c = conn();
        let id = create_rule(&c, "TITLE", "quantum", "DENY").unwrap();
        let rules = list_rules(&c).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].rule_id, id);
        assert!(rules[0].enabled);

        assert!(delete_rule(&c, id).unwrap());
        assert!(list_rules(&c).unwrap().is_empty());
        assert!(!delete_rule(&c, id).unwrap());
    }

    #[test]
    fn deny_matches_all_keywords_and_allow_overrides() {
        let rules = vec![
            FilterRule {
                rule_id: 1,
                field: "TITLE".into(),
                keywords: "AI, quantum computing".into(),
                action: "DENY".into(),
                enabled: true,
            },
            FilterRule {
                rule_id: 2,
                field: "SUMMARY".into(),
                keywords: "ML".into(),
                action: "ALLOW".into(),
                enabled: true,
            },
        ];
        assert!(is_hidden(
            &rules,
            "New AI results in quantum computing",
            "nothing else here",
            ""
        ));
        assert!(!is_hidden(
            &rules,
            "New AI results in quantum computing",
            "we also use ML here",
            ""
        ));
        // Only one of the two DENY keywords present -> AND means it doesn't fire.
        assert!(!is_hidden(&rules, "New AI results only", "irrelevant", ""));
    }

    #[test]
    fn separators_only_keywords_never_match() {
        let rules = vec![FilterRule {
            rule_id: 1,
            field: "TITLE".into(),
            keywords: " , ".into(),
            action: "DENY".into(),
            enabled: true,
        }];
        assert!(!is_hidden(&rules, "Anything at all", "", ""));
    }

    fn entry(dedup_key: &str, source_id: Option<&str>) -> CacheEntry {
        CacheEntry {
            dedup_key: dedup_key.to_string(),
            source_id: source_id.map(|s| s.to_string()),
            entry_json: format!(r#"{{"title":"{dedup_key}"}}"#),
            published_at: None,
        }
    }

    /// Backdate a row's FETCHED_AT (the retention cutoff -- see `load_cache_entries`)
    /// to simulate an old cache entry; `merge_cache_entries` always writes it as "now".
    fn age_row(c: &Connection, dedup_key: &str, days_old: i64) {
        c.execute(
            "UPDATE RSS_CACHE_ENTRY SET FETCHED_AT = datetime('now', '-' || ?1 || ' days')
             WHERE DEDUP_KEY = ?2",
            params![days_old, dedup_key],
        )
        .unwrap();
    }

    #[test]
    fn merge_is_additive_and_never_overwrites() {
        let mut c = conn();
        merge_cache_entries(&mut c, "u", &[entry("a", None)]).unwrap();
        // Same dedup_key again, different payload -- must be ignored, not overwritten.
        let mut changed = entry("a", None);
        changed.entry_json = r#"{"title":"changed"}"#.to_string();
        merge_cache_entries(&mut c, "u", &[changed, entry("b", None)]).unwrap();

        let loaded = load_cache_entries(&c, "u", 30).unwrap();
        assert_eq!(loaded.len(), 2);
        let titles: HashSet<_> = loaded
            .iter()
            .filter_map(|e| e.get("title").and_then(|t| t.as_str()))
            .collect();
        assert!(titles.contains("a"), "original row must survive untouched");
        assert!(!titles.contains("changed"));
        assert!(titles.contains("b"));
    }

    #[test]
    fn window_excludes_old_entries_even_when_permanently_dismissed() {
        let mut c = conn();
        merge_cache_entries(
            &mut c,
            "u",
            &[
                entry("recent", None),
                entry("stale", None),
                entry("stale-and-blocked", Some("arxiv:1")),
            ],
        )
        .unwrap();
        age_row(&c, "stale", 60);
        age_row(&c, "stale-and-blocked", 60);
        dismiss(&c, "arxiv:1", 1, true).unwrap();

        let loaded = load_cache_entries(&c, "u", 30).unwrap();
        let titles: HashSet<_> = loaded
            .iter()
            .filter_map(|e| e.get("title").and_then(|t| t.as_str()))
            .collect();
        assert!(titles.contains("recent"));
        assert!(
            !titles.contains("stale"),
            "outside the window -- must not load"
        );
        assert!(
            !titles.contains("stale-and-blocked"),
            "dismissal doesn't exempt a row from the window -- ages out like any other"
        );
    }

    #[test]
    fn prune_drops_all_stale_rows_regardless_of_dismissal() {
        let mut c = conn();
        merge_cache_entries(
            &mut c,
            "u",
            &[
                entry("recent", None),
                entry("stale", None),
                entry("stale-and-blocked", Some("arxiv:1")),
            ],
        )
        .unwrap();
        age_row(&c, "stale", 60);
        age_row(&c, "stale-and-blocked", 60);
        dismiss(&c, "arxiv:1", 1, true).unwrap();

        let pruned = prune_cache_entries(&c, "u", 30).unwrap();
        assert_eq!(pruned, 2);

        let remaining: HashSet<String> = c
            .prepare("SELECT DEDUP_KEY FROM RSS_CACHE_ENTRY")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(remaining.contains("recent"));
        assert!(!remaining.contains("stale"));
        assert!(!remaining.contains("stale-and-blocked"));
    }

    #[test]
    fn disabled_rule_never_matches() {
        let rules = vec![FilterRule {
            rule_id: 1,
            field: "TITLE".into(),
            keywords: "spam".into(),
            action: "DENY".into(),
            enabled: false,
        }];
        assert!(!is_hidden(&rules, "spam spam spam", "", ""));
    }
}
