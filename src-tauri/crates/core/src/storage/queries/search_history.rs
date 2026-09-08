//! Recent-search-term autocomplete. The enabled flag and max-size cap live in
//! user_settings, so the caller gates `add_term` on the flag and passes
//! `max_history`; these functions are pure DB. Term/prefix trimming is done here.

use rusqlite::{params, Connection};

use crate::error::Result;

/// Upsert TERM (exact, case-sensitive `UNIQUE`), bump USE_COUNT, then
/// prune to the `max_history` most-recently-used rows. A blank term is a no-op.
pub fn add_term(conn: &Connection, term: &str, max_history: i64) -> Result<()> {
    let stripped = term.trim();
    if stripped.is_empty() {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO SEARCH_HISTORY (TERM, USE_COUNT, LAST_USED_AT) \
         VALUES (?1, 1, datetime('now')) \
         ON CONFLICT(TERM) DO UPDATE SET \
            USE_COUNT    = USE_COUNT + 1, \
            LAST_USED_AT = datetime('now')",
        params![stripped],
    )?;
    conn.execute(
        "DELETE FROM SEARCH_HISTORY WHERE HISTORY_ID NOT IN ( \
            SELECT HISTORY_ID FROM SEARCH_HISTORY \
            ORDER BY LAST_USED_AT DESC, USE_COUNT DESC LIMIT ?1)",
        params![max_history],
    )?;
    Ok(())
}

/// Up to `limit` terms `LIKE <prefix>%` (case-insensitive),
/// ranked by USE_COUNT desc then recency desc. A blank prefix → `[]`.
pub fn get_suggestions(conn: &Connection, prefix: &str, limit: i64) -> Result<Vec<String>> {
    let stripped = prefix.trim();
    if stripped.is_empty() {
        return Ok(Vec::new());
    }
    let pattern = format!("{stripped}%");
    let mut stmt = conn.prepare(
        "SELECT TERM FROM SEARCH_HISTORY WHERE TERM LIKE ?1 COLLATE NOCASE \
         ORDER BY USE_COUNT DESC, LAST_USED_AT DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![pattern, limit], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<String>>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{self, db};

    fn conn() -> Connection {
        let c = db::open_in_memory().unwrap();
        storage::init_db(&c).unwrap();
        c
    }

    #[test]
    fn blank_term_and_prefix_are_noops() {
        let c = conn();
        add_term(&c, "   ", 200).unwrap();
        let cnt: i64 = c
            .query_row("SELECT COUNT(*) FROM SEARCH_HISTORY", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cnt, 0);
        assert_eq!(get_suggestions(&c, "  ", 10).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn prefix_match_is_case_insensitive_ranked_by_use_count() {
        let c = conn();
        add_term(&c, "Manifold learning", 200).unwrap();
        add_term(&c, "manifold hypothesis", 200).unwrap();
        add_term(&c, "manifold hypothesis", 200).unwrap(); // use_count 2 → ranks first
        add_term(&c, "quantum", 200).unwrap();

        let s = get_suggestions(&c, "man", 10).unwrap();
        assert_eq!(
            s,
            vec![
                "manifold hypothesis".to_string(),
                "Manifold learning".to_string()
            ]
        );
        // case-insensitive prefix
        assert_eq!(get_suggestions(&c, "MAN", 10).unwrap().len(), 2);
        // exact, case-sensitive dedup: the two distinct-case "manifold ..." stay distinct rows
        let total: i64 = c
            .query_row("SELECT COUNT(*) FROM SEARCH_HISTORY", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 3);
    }

    #[test]
    fn prune_keeps_only_max_history_most_recent() {
        let c = conn();
        for t in ["a", "b", "c"] {
            add_term(&c, t, 2).unwrap(); // cap 2 → oldest pruned each insert
        }
        let cnt: i64 = c
            .query_row("SELECT COUNT(*) FROM SEARCH_HISTORY", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cnt, 2);
    }
}
