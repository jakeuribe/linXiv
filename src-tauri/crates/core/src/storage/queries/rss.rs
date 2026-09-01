//! RSS_PAPER_ROOTS / RSS_PAPER / RSS_FILTER_RULE queries: seen/dismissed feed
//! entries and the keyword rules that auto-hide entries before the client sees them.

use std::collections::HashSet;

use chrono::NaiveDateTime;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

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

/// Hide a feed entry. `permanent` blocks the whole paper forever
/// (`RSS_PAPER_ROOTS.REMOVAL_TYPE = 'DOI'`); otherwise dismisses just this
/// version (`RSS_PAPER.REMOVAL_TYPE = 'VER'`) -- a later version resurfaces.
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

// ponytail: not yet user-configurable (no settings UI wired up) -- hardcoded
// until there's a reason to expose them.
const VER_DISMISS_PRUNE_HOURS: i64 = 48;
const DOI_DISMISS_PRUNE_DAYS: i64 = 730;

/// Forget old `RSS_PAPER`/`RSS_PAPER_ROOTS` bookkeeping -- VER/DOI dismissals
/// and plain 'NOT' seen rows, none of which anything reads back once stale.
pub fn prune_dismissed(conn: &Connection, cache_retention_days: i64) -> Result<()> {
    // Floored at cache_retention_days: a dismissal can't be forgotten while
    // the RSS_CACHE_ENTRY row it hides is still served, or it reappears silently.
    let ver_cutoff_hours = VER_DISMISS_PRUNE_HOURS.max(cache_retention_days.saturating_mul(24));
    conn.execute(
        "DELETE FROM RSS_PAPER
         WHERE REMOVAL_TYPE = 'VER'
           AND REMOVED_AT < datetime('now', '-' || ?1 || ' hours')",
        [ver_cutoff_hours],
    )?;
    // 'NOT' (never-dismissed) rows: nothing reads them once the cache window
    // that "seen" was tracking against has itself moved past them.
    conn.execute(
        "DELETE FROM RSS_PAPER
         WHERE REMOVAL_TYPE = 'NOT'
           AND CREATED_AT < datetime('now', '-' || ?1 || ' days')",
        [cache_retention_days],
    )?;
    // Floored like the VER cutoff above -- same reappearance risk if retention
    // is ever set past the DOI default.
    let doi_cutoff_days = DOI_DISMISS_PRUNE_DAYS.max(cache_retention_days);
    conn.execute(
        "DELETE FROM RSS_PAPER_ROOTS
         WHERE REMOVAL_TYPE = 'DOI'
           AND REMOVED_AT < datetime('now', '-' || ?1 || ' days')",
        [doi_cutoff_days],
    )?;
    // Orphaned 'NOT' root: only once no RSS_PAPER child references it (cascade
    // only flows parent->child, so a stale child alone won't take this with it).
    conn.execute(
        "DELETE FROM RSS_PAPER_ROOTS
         WHERE REMOVAL_TYPE = 'NOT'
           AND CREATED_AT < datetime('now', '-' || ?1 || ' days')
           AND NOT EXISTS (
               SELECT 1 FROM RSS_PAPER WHERE RSS_PAPER.SOURCE_FK = RSS_PAPER_ROOTS.SOURCE_FK
           )",
        [cache_retention_days],
    )?;
    Ok(())
}

/// A freshly-fetched feed entry ready to persist into `RSS_CACHE_ENTRY`.
/// `dedup_key` is arxiv `id+version` when available, else link/title (see
/// `feed.rs::to_cache_entry`). `source_id` is stored for reference only.
pub struct CacheEntry {
    pub dedup_key: String,
    pub source_id: Option<String>,
    pub entry_json: String,
    pub published_at: Option<NaiveDateTime>,
}

/// Additively merge fresh entries into the per-URL cache: insert only unseen
/// dedup keys, never overwrite an existing row. One transaction for the batch.
pub fn merge_cache_entries(
    conn: &mut Connection,
    feed_url: &str,
    entries: &[CacheEntry],
) -> Result<()> {
    let tx = conn.transaction()?;
    {
        let mut insert = tx.prepare_cached(
            "INSERT OR IGNORE INTO RSS_CACHE_ENTRY
                 (FEED_URL, DEDUP_KEY, SOURCE_ID, ENTRY_JSON, PUBLISHED_AT)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        for e in entries {
            insert.execute(params![
                feed_url,
                e.dedup_key,
                e.source_id,
                e.entry_json,
                // Space-separated to match datetime('now')'s format -- sorted as a raw
                // string below, so 'T' would sort newer than same-day ' '-form times.
                e.published_at
                    .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string()),
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Cap on rows loaded per feed GET -- above the 200 `annotate_and_filter` keeps.
const MAX_LOADED_ENTRIES: i64 = 500;

/// Cached entries for `feed_url` within the retention window, newest-published-
/// first. Windowed by `FETCHED_AT`, not `PUBLISHED_AT` -- a just-fetched v2/v3
/// of an old paper must not age out on the fetch that added it. Malformed
/// stored JSON is logged and skipped rather than failing the whole response.
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

/// `RSS_FILTER_RULE.FIELD` -- closed set, so a real enum instead of `String`
/// keeps `FilterRule` codegen-able (a bare `String` would flatten the
/// frontend's hand-written `"TITLE"|"SUMMARY"|"AUTHOR"` union to `string`).
/// `rename_all` pins the wire strings to the on-disk `TEXT` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FilterField {
    Title,
    Summary,
    Author,
}

impl FilterField {
    /// The canonical on-disk `TEXT` value (matches the wire string).
    pub const fn as_str(self) -> &'static str {
        match self {
            FilterField::Title => "TITLE",
            FilterField::Summary => "SUMMARY",
            FilterField::Author => "AUTHOR",
        }
    }
}

impl rusqlite::types::ToSql for FilterField {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}

impl rusqlite::types::FromSql for FilterField {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        match value.as_str()? {
            "TITLE" => Ok(FilterField::Title),
            "SUMMARY" => Ok(FilterField::Summary),
            "AUTHOR" => Ok(FilterField::Author),
            _ => Err(rusqlite::types::FromSqlError::InvalidType),
        }
    }
}

/// `RSS_FILTER_RULE.ACTION` -- see `FilterField` for why this is an enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FilterAction {
    Deny,
    Allow,
}

impl FilterAction {
    /// The canonical on-disk `TEXT` value (matches the wire string).
    pub const fn as_str(self) -> &'static str {
        match self {
            FilterAction::Deny => "DENY",
            FilterAction::Allow => "ALLOW",
        }
    }
}

impl rusqlite::types::ToSql for FilterAction {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}

impl rusqlite::types::FromSql for FilterAction {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        match value.as_str()? {
            "DENY" => Ok(FilterAction::Deny),
            "ALLOW" => Ok(FilterAction::Allow),
            _ => Err(rusqlite::types::FromSqlError::InvalidType),
        }
    }
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct FilterRule {
    pub rule_id: i64,
    pub field: FilterField,
    pub keywords: String,
    pub action: FilterAction,
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

pub fn create_rule(
    conn: &Connection,
    field: FilterField,
    keywords: &str,
    action: FilterAction,
) -> Result<i64> {
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

fn field_value<'a>(
    field: FilterField,
    title: &'a str,
    summary: &'a str,
    authors: &'a str,
) -> &'a str {
    match field {
        FilterField::Title => title,
        FilterField::Summary => summary,
        FilterField::Author => authors,
    }
}

/// A `FilterRule` with keywords pre-split/trimmed/lowercased and rules that
/// can never match (disabled, or no real keywords) dropped, so per-entry
/// matching does no re-parsing. Compile once per rule set, match per entry.
pub struct CompiledRule {
    field: FilterField,
    action: FilterAction,
    keywords: Vec<String>,
}

pub fn compile_rules(rules: Vec<FilterRule>) -> Vec<CompiledRule> {
    rules
        .into_iter()
        .filter(|r| r.enabled)
        .filter_map(|r| {
            let keywords: Vec<String> = r
                .keywords
                .split(',')
                .map(|k| k.trim().to_lowercase())
                .filter(|k| !k.is_empty())
                .collect();
            // Separators-only keywords (e.g. ",") would make `.all()` vacuously
            // true and match every entry -- require at least one real keyword.
            (!keywords.is_empty()).then_some(CompiledRule {
                field: r.field,
                action: r.action,
                keywords,
            })
        })
        .collect()
}

/// True if a DENY rule matches (all keywords, case-insensitive substring)
/// and no ALLOW rule also matches. Rules come from `compile_rules`.
pub fn is_hidden(rules: &[CompiledRule], title: &str, summary: &str, authors: &str) -> bool {
    if rules.is_empty() {
        return false;
    }
    // Lowercase each field at most once, and only when some rule targets it.
    let lower = |f: FilterField| {
        if rules.iter().any(|r| r.field == f) {
            field_value(f, title, summary, authors).to_lowercase()
        } else {
            String::new()
        }
    };
    let title = lower(FilterField::Title);
    let summary = lower(FilterField::Summary);
    let authors = lower(FilterField::Author);
    let rule_matches = |r: &CompiledRule| -> bool {
        let field = field_value(r.field, &title, &summary, &authors);
        r.keywords.iter().all(|kw| field.contains(kw.as_str()))
    };
    let denied = rules
        .iter()
        .filter(|r| r.action == FilterAction::Deny)
        .any(rule_matches);
    denied
        && !rules
            .iter()
            .filter(|r| r.action == FilterAction::Allow)
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

    fn backdate_removed_at(c: &Connection, table: &str, source_id: &str, hours_old: i64) {
        c.execute(
            &format!(
                "UPDATE {table} SET REMOVED_AT = datetime('now', '-' || ?1 || ' hours')
                 WHERE SOURCE_ID = ?2"
            ),
            params![hours_old, source_id],
        )
        .unwrap();
    }

    fn backdate_created_at(c: &Connection, table: &str, source_id: &str, days_old: i64) {
        c.execute(
            &format!(
                "UPDATE {table} SET CREATED_AT = datetime('now', '-' || ?1 || ' days')
                 WHERE SOURCE_ID = ?2"
            ),
            params![days_old, source_id],
        )
        .unwrap();
    }

    #[test]
    fn prune_dismissed_forgets_stale_ver_but_keeps_fresh() {
        let c = conn();
        upsert_seen(&c, "arxiv:2401.00010", 1, "old").unwrap();
        dismiss(&c, "arxiv:2401.00010", 1, false).unwrap();
        backdate_removed_at(
            &c,
            "RSS_PAPER",
            "arxiv:2401.00010",
            VER_DISMISS_PRUNE_HOURS + 1,
        );

        upsert_seen(&c, "arxiv:2401.00011", 1, "fresh").unwrap();
        dismiss(&c, "arxiv:2401.00011", 1, false).unwrap();

        prune_dismissed(&c, 1).unwrap();
        let dismissed = dismissed_versions(&c).unwrap();
        assert!(!dismissed.contains(&("arxiv:2401.00010".to_string(), 1)));
        assert!(dismissed.contains(&("arxiv:2401.00011".to_string(), 1)));
    }

    /// A VER dismissal must outlive its `RSS_CACHE_ENTRY` row, or the entry
    /// silently reappears once the raw VER_DISMISS_PRUNE_HOURS cutoff passes.
    #[test]
    fn prune_dismissed_floors_ver_cutoff_at_cache_retention() {
        let c = conn();
        upsert_seen(&c, "arxiv:2401.00012", 1, "old-ish").unwrap();
        dismiss(&c, "arxiv:2401.00012", 1, false).unwrap();
        // Past the raw 48h constant, nowhere near a 30-day retention window.
        backdate_removed_at(
            &c,
            "RSS_PAPER",
            "arxiv:2401.00012",
            VER_DISMISS_PRUNE_HOURS + 1,
        );

        prune_dismissed(&c, 30).unwrap();
        assert!(dismissed_versions(&c)
            .unwrap()
            .contains(&("arxiv:2401.00012".to_string(), 1)));
    }

    #[test]
    fn prune_dismissed_floors_doi_cutoff_at_cache_retention() {
        let c = conn();
        upsert_seen(&c, "arxiv:2401.00013", 1, "old-ish, blocked").unwrap();
        dismiss(&c, "arxiv:2401.00013", 1, true).unwrap();
        // Past the raw 730-day constant, nowhere near an 830-day retention window.
        backdate_removed_at(
            &c,
            "RSS_PAPER_ROOTS",
            "arxiv:2401.00013",
            (DOI_DISMISS_PRUNE_DAYS + 1) * 24,
        );

        prune_dismissed(&c, DOI_DISMISS_PRUNE_DAYS + 100).unwrap();
        assert!(blocked_source_ids(&c).unwrap().contains("arxiv:2401.00013"));
    }

    #[test]
    fn prune_dismissed_forgets_stale_doi_and_cascades_to_paper_rows() {
        let c = conn();
        upsert_seen(&c, "arxiv:2401.00020", 1, "old blocked").unwrap();
        dismiss(&c, "arxiv:2401.00020", 1, true).unwrap();
        backdate_removed_at(
            &c,
            "RSS_PAPER_ROOTS",
            "arxiv:2401.00020",
            DOI_DISMISS_PRUNE_DAYS * 24 + 1,
        );

        upsert_seen(&c, "arxiv:2401.00021", 1, "fresh blocked").unwrap();
        dismiss(&c, "arxiv:2401.00021", 1, true).unwrap();

        prune_dismissed(&c, 1).unwrap();
        let blocked = blocked_source_ids(&c).unwrap();
        assert!(!blocked.contains("arxiv:2401.00020"));
        assert!(blocked.contains("arxiv:2401.00021"));

        // ON DELETE CASCADE must have taken the RSS_PAPER row with it.
        let remaining: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM RSS_PAPER WHERE SOURCE_ID = 'arxiv:2401.00020'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
    }

    /// A fresh DOI root must survive delete #2 pruning its own 'NOT' child --
    /// delete #4's REMOVAL_TYPE='NOT' filter must never reach a DOI root.
    #[test]
    fn prune_dismissed_keeps_fresh_doi_root_after_its_not_child_is_pruned() {
        let c = conn();
        upsert_seen(&c, "arxiv:2401.00022", 1, "blocked, old seen row").unwrap();
        backdate_created_at(&c, "RSS_PAPER", "arxiv:2401.00022", 31);
        dismiss(&c, "arxiv:2401.00022", 1, true).unwrap();

        prune_dismissed(&c, 30).unwrap();

        assert!(blocked_source_ids(&c).unwrap().contains("arxiv:2401.00022"));
        let remaining: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM RSS_PAPER WHERE SOURCE_ID = 'arxiv:2401.00022'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0, "the 'NOT' child should still be pruned");
    }

    #[test]
    fn prune_dismissed_forgets_stale_seen_rows_but_keeps_fresh() {
        let c = conn();
        upsert_seen(&c, "arxiv:2401.00030", 1, "old, never dismissed").unwrap();
        backdate_created_at(&c, "RSS_PAPER_ROOTS", "arxiv:2401.00030", 31);
        backdate_created_at(&c, "RSS_PAPER", "arxiv:2401.00030", 31);

        upsert_seen(&c, "arxiv:2401.00031", 1, "fresh, never dismissed").unwrap();

        prune_dismissed(&c, 30).unwrap();

        let paper_count: i64 = c
            .query_row("SELECT COUNT(*) FROM RSS_PAPER", [], |r| r.get(0))
            .unwrap();
        assert_eq!(paper_count, 1, "only the fresh seen row should survive");
        let roots_count: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM RSS_PAPER_ROOTS WHERE SOURCE_ID = 'arxiv:2401.00030'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            roots_count, 0,
            "orphaned root with no remaining children must go too"
        );
    }

    /// The EXISTS guard must not orphan-delete a root while a live child
    /// (not yet stale) still references it.
    #[test]
    fn prune_dismissed_keeps_old_root_with_a_live_child() {
        let c = conn();
        upsert_seen(&c, "arxiv:2401.00040", 1, "old root, fresh dismissal").unwrap();
        backdate_created_at(&c, "RSS_PAPER_ROOTS", "arxiv:2401.00040", 31);
        dismiss(&c, "arxiv:2401.00040", 1, false).unwrap();

        prune_dismissed(&c, 30).unwrap();

        assert!(dismissed_versions(&c)
            .unwrap()
            .contains(&("arxiv:2401.00040".to_string(), 1)));
        let roots_count: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM RSS_PAPER_ROOTS WHERE SOURCE_ID = 'arxiv:2401.00040'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(roots_count, 1, "root must survive while its child does");
    }

    #[test]
    fn rule_crud_round_trips() {
        let c = conn();
        let id = create_rule(&c, FilterField::Title, "quantum", FilterAction::Deny).unwrap();
        let rules = list_rules(&c).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].rule_id, id);
        assert_eq!(rules[0].field, FilterField::Title);
        assert_eq!(rules[0].keywords, "quantum");
        assert_eq!(rules[0].action, FilterAction::Deny);
        assert!(rules[0].enabled);

        // ToSql must write the exact canonical TEXT values, not Debug names.
        let (f, a): (String, String) = c
            .query_row(
                "SELECT FIELD, ACTION FROM RSS_FILTER_RULE WHERE RULE_ID = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((f.as_str(), a.as_str()), ("TITLE", "DENY"));

        assert!(delete_rule(&c, id).unwrap());
        assert!(list_rules(&c).unwrap().is_empty());
        assert!(!delete_rule(&c, id).unwrap());
    }

    #[test]
    fn deny_matches_all_keywords_and_allow_overrides() {
        let rules = compile_rules(vec![
            FilterRule {
                rule_id: 1,
                field: FilterField::Title,
                keywords: "AI, quantum computing".into(),
                action: FilterAction::Deny,
                enabled: true,
            },
            FilterRule {
                rule_id: 2,
                field: FilterField::Summary,
                keywords: "ML".into(),
                action: FilterAction::Allow,
                enabled: true,
            },
        ]);
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
        let rules = compile_rules(vec![FilterRule {
            rule_id: 1,
            field: FilterField::Title,
            keywords: " , ".into(),
            action: FilterAction::Deny,
            enabled: true,
        }]);
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

    /// Backdate a row's FETCHED_AT to simulate an old cache entry.
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
        let rules = compile_rules(vec![FilterRule {
            rule_id: 1,
            field: FilterField::Title,
            keywords: "spam".into(),
            action: FilterAction::Deny,
            enabled: false,
        }]);
        assert!(!is_hidden(&rules, "spam spam spam", "", ""));
    }
}
