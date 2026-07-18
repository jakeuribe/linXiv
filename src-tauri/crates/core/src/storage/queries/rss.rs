//! RSS_PAPER_ROOTS / RSS_PAPER / RSS_FILTER_RULE queries — persisted state for the
//! home RSS feed: which entries have been seen and dismissed, and the keyword
//! rules that auto-hide entries before they reach the client.

use std::collections::HashSet;

use rusqlite::{params, Connection};
use serde::Serialize;

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
