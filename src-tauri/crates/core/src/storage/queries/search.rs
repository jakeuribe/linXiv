use std::collections::HashMap;

use rusqlite::Connection;

use crate::error::Result;
use crate::models::PaperDetails;

/// `storage/db.py::search_full_text` — FTS5 over TeX source AND note content.
/// Returns the latest version of each matching paper, ranked by bm25 (lower =
/// better); a paper matched by both its full text and a note takes its best score.
///
/// FTS misnomer: `papers_fts.paper_id` holds the SOURCE_ID *string*, so that half
/// joins on `latest_papers.source_id = papers_fts.paper_id` (NOT the int PAPER_ID).
/// notes_fts carries SOURCE_FK, joined back through PAPER_ROOTS to the same SOURCE_ID.
/// init_db always creates both FTS tables, so Python's "table missing" guard is moot.
///
/// `query` is the raw search box input; `match_expr` turns it into FTS5 syntax.
/// Each branch runs as its own prepared statement, and a prepare/query error
/// from either is treated as "no matches from that branch" rather than aborting
/// the whole search. `match_expr` emits nothing schema-specific; the fallback
/// stays as a backstop for an index that is missing or corrupt.
pub fn search_full_text(conn: &Connection, query: &str, limit: i64) -> Result<Vec<PaperDetails>> {
    let limit = limit.clamp(0, 1000);
    let Some(expr) = match_expr(query) else {
        return Ok(Vec::new());
    };
    let mut best: HashMap<String, f64> = HashMap::new();
    for (sid, score) in fts_matches(
        conn,
        "SELECT fts.paper_id, bm25(papers_fts) FROM papers_fts fts WHERE papers_fts MATCH ?1",
        &expr,
    )
    .into_iter()
    .chain(fts_matches(
        conn,
        "SELECT r.SOURCE_ID, bm25(notes_fts) FROM notes_fts \
         JOIN PAPER_ROOTS r ON r.SOURCE_FK = notes_fts.source_fk \
         WHERE notes_fts MATCH ?1 AND r.STATUS = 'active'",
        &expr,
    )) {
        best.entry(sid)
            .and_modify(|s| {
                if score < *s {
                    *s = score;
                }
            })
            .or_insert(score);
    }

    let mut scored: Vec<(String, f64)> = best.into_iter().collect();
    scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit as usize);
    if scored.is_empty() {
        return Ok(Vec::new());
    }

    let sql = format!(
        "SELECT {} FROM latest_papers WHERE source_id IN ({})",
        super::paper::PAPER_COLUMNS_NO_TEXT,
        vec!["?"; scored.len()].join(", ")
    );
    let mut stmt = conn.prepare(&sql)?;
    let ids: Vec<&str> = scored.iter().map(|(sid, _)| sid.as_str()).collect();
    // Manual loop (not query_map): row_to_paper returns CoreError on a bad
    // DATE/LIST decode, which query_map's rusqlite::Result closure can't carry.
    let mut rows = stmt.query(rusqlite::params_from_iter(ids))?;
    let mut by_source_id: HashMap<String, PaperDetails> = HashMap::new();
    while let Some(row) = rows.next()? {
        let details = super::paper::row_to_paper(row)?;
        by_source_id.insert(details.source_id.clone(), details);
    }
    Ok(scored
        .into_iter()
        .filter_map(|(sid, _)| by_source_id.remove(&sid))
        .collect())
}

/// Rewrite raw search box input into an FTS5 MATCH expression.
///
/// FTS5's query language reads `-` and `:` as column-filter syntax and rejects
/// stray punctuation outright, so `encoder-decoder` parses as a filter on a
/// column named `decoder` and `c++` is a syntax error near `+`. Both raise, and
/// `fts_matches` reports a raise as "no rows" — so before this, a hyphenated
/// query looked like a search that legitimately found nothing.
///
/// Each bare word becomes a quoted phrase, which FTS5 splits with the same
/// tokenizer that split the document, so `encoder-decoder` matches the
/// document's `encoder-decoder`. The syntax that already worked is preserved:
/// double-quoted phrases, the AND/OR/NOT operators, and a trailing `*`.
///
/// `None` when nothing searchable is left — an empty MATCH is itself an error.
///
/// `ponytail: NEAR()/^ are not preserved (they'd need a real parser); they
/// fall through to a literal term search.`
fn match_expr(raw: &str) -> Option<String> {
    // FTS5 reads its query as a C string; an embedded NUL truncates it before
    // the closing quote is seen, so strip NUL up front.
    let raw: String = raw.chars().filter(|&c| c != '\0').collect();
    let mut out: Vec<String> = Vec::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        if c == '"' {
            // A phrase the user quoted: take everything up to the closing quote,
            // or to end of input if they never typed one.
            let mut phrase = String::new();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                phrase.push(c);
            }
            let prefix = chars.next_if_eq(&'*').is_some();
            push_term(&mut out, &phrase, prefix);
            continue;
        }
        let mut tok = String::from(c);
        while let Some(&next) = chars.peek() {
            if next.is_whitespace() || next == '"' {
                break;
            }
            tok.push(next);
            chars.next();
        }
        let prefix = tok.ends_with('*');
        let body = tok.strip_suffix('*').unwrap_or(&tok);
        if !prefix && matches!(body, "AND" | "OR" | "NOT") {
            // Two operators in a row (or a leading one) is a syntax error.
            if !out.is_empty() && !is_operator(out.last()) {
                out.push(body.to_string());
            }
        } else {
            push_term(&mut out, body, prefix);
        }
    }
    if is_operator(out.last()) {
        out.pop();
    }
    (!out.is_empty()).then(|| out.join(" "))
}

fn is_operator(tok: Option<&String>) -> bool {
    matches!(tok.map(String::as_str), Some("AND" | "OR" | "NOT"))
}

/// Push one term as a quoted FTS5 phrase.
fn push_term(out: &mut Vec<String>, term: &str, prefix: bool) {
    // FTS5 drops punctuation when tokenizing, so an all-punctuation term holds
    // no token to match and would emit `""` — itself a syntax error.
    if !term.chars().any(char::is_alphanumeric) {
        return;
    }
    out.push(if prefix {
        format!("\"{term}\"*")
    } else {
        format!("\"{term}\"")
    });
}

/// Run one FTS MATCH query, returning (source_id, bm25 score) pairs. A prepare
/// or execute error (a missing or corrupt index) yields an empty result instead
/// of propagating, so one unusable index doesn't zero out the other's hits.
fn fts_matches(conn: &Connection, sql: &str, query: &str) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    let mut stmt = match conn.prepare(sql) {
        Ok(stmt) => stmt,
        Err(e) => {
            tracing::warn!("fts_matches prepare failed for {sql:?}: {e}");
            return out;
        }
    };
    let mut rows = match stmt.query((query,)) {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!("fts_matches query failed for {sql:?}: {e}");
            return out;
        }
    };
    while let Ok(Some(row)) = rows.next() {
        let (Ok(sid), Ok(score)) = (row.get::<_, String>(0), row.get::<_, f64>(1)) else {
            continue;
        };
        out.push((sid, score));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{self, db};
    use chrono::NaiveDate;

    fn seed(conn: &Connection, source_id: &str, full_text: &str) {
        conn.execute(
            "INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES (?1)",
            [source_id],
        )
        .unwrap();
        let source_fk: i64 = conn
            .query_row(
                "SELECT SOURCE_FK FROM PAPER_ROOTS WHERE SOURCE_ID = ?1",
                [source_id],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, CATEGORY, HAS_PDF, SOURCE_FK) \
             VALUES (?1, 1, ?2, 'cs.LG', 1, ?3)",
            rusqlite::params![source_id, "A Title", source_fk],
        )
        .unwrap();
        let paper_id: i64 = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO PAPER_META (PAPER_ID, PUBLISHED, AUTHORS, TAGS, SUMMARY, FULL_TEXT) \
             VALUES (?1, '2024-03-05', '[\"Ada\"]', '[\"ml\"]', 'sum', ?2)",
            rusqlite::params![paper_id, full_text],
        )
        .unwrap();
        // No hand-insert into papers_fts: the PAPER_META write above fires the
        // sync trigger, which derives the row. Seeding it again would give every
        // fixture paper two index rows and stop these tests running against the
        // one-row-per-paper shape production actually has.
    }

    #[test]
    fn matches_on_tex_source_and_maps_fields() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        seed(
            &conn,
            "arxiv:2204.12985",
            "the manifold hypothesis in latent space",
        );
        seed(
            &conn,
            "arxiv:1111.00000",
            "unrelated quantum chromodynamics",
        );

        let hits = search_full_text(&conn, "manifold", 20).unwrap();
        assert_eq!(hits.len(), 1);
        let p = &hits[0];
        assert_eq!(p.source_id, "arxiv:2204.12985");
        assert_eq!(p.version, 1);
        assert_eq!(p.title, "A Title");
        assert_eq!(p.published, NaiveDate::from_ymd_opt(2024, 3, 5));
        assert_eq!(p.authors, vec!["Ada".to_string()]);
        assert_eq!(p.tags, vec!["ml".to_string()]);
        assert!(p.has_pdf);

        assert_eq!(
            search_full_text(&conn, "nonexistentterm", 20)
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn matches_on_note_content() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        seed(&conn, "arxiv:2204.12985", "some tex source");

        // A note whose distinctive term appears in no paper's full text; the
        // notes_fts AFTER-INSERT trigger indexes it, so the FTS path finds it.
        conn.execute(
            "INSERT INTO NOTE (SOURCE_FK, TITLE, NOTE) \
             SELECT SOURCE_FK, 'n', 'zephyranthes reminder' \
             FROM PAPER_ROOTS WHERE SOURCE_ID = ?1",
            ["arxiv:2204.12985"],
        )
        .unwrap();

        let hits = search_full_text(&conn, "zephyranthes", 20).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source_id, "arxiv:2204.12985");
    }

    #[test]
    fn dedupes_paper_matched_in_both_indexes() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        seed(
            &conn,
            "arxiv:2204.12985",
            "the manifold hypothesis in latent space",
        );
        conn.execute(
            "INSERT INTO NOTE (SOURCE_FK, TITLE, NOTE) \
             SELECT SOURCE_FK, 'n', 'manifold reading notes' \
             FROM PAPER_ROOTS WHERE SOURCE_ID = ?1",
            ["arxiv:2204.12985"],
        )
        .unwrap();

        let hits = search_full_text(&conn, "manifold", 20).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source_id, "arxiv:2204.12985");
    }

    /// Punctuation FTS5 reads as syntax is searched for literally instead. Each
    /// of these raised before, and a raise reads as "no matches" — so the search
    /// silently returned nothing for terms that are all over a TeX corpus.
    #[test]
    fn punctuation_in_a_query_searches_instead_of_raising() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        seed(
            &conn,
            "arxiv:2204.12985",
            "the encoder-decoder model is state-of-the-art for c++ at 50% recall",
        );
        seed(
            &conn,
            "arxiv:1111.00000",
            "unrelated quantum chromodynamics",
        );

        for q in [
            "encoder-decoder",
            "state-of-the-art",
            "c++",
            "50%",
            "-encoder",
            "recall:",
            "(encoder",
            "encoder-decoder AND c++",
            "encod*",
        ] {
            let hits = search_full_text(&conn, q, 20).unwrap();
            assert_eq!(hits.len(), 1, "query {q:?} found {} papers", hits.len());
            assert_eq!(hits[0].source_id, "arxiv:2204.12985", "query {q:?}");
        }
    }

    /// A query with no searchable token can't become a MATCH expression (an
    /// empty one raises), so it returns no results rather than erroring.
    #[test]
    fn a_query_with_nothing_to_search_returns_no_results() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        seed(&conn, "arxiv:2204.12985", "the manifold hypothesis");

        for q in ["", "   ", "-", "%%%", "AND", "OR OR"] {
            assert!(
                search_full_text(&conn, q, 20).unwrap().is_empty(),
                "query {q:?} should find nothing"
            );
        }
    }

    /// Column-filter syntax (`full_text:foo`) used to reach FTS5 and is now read
    /// as literal text — the trade for making hyphens work. It finds papers
    /// whose text holds those words, and nothing when it doesn't.
    #[test]
    fn column_filter_syntax_is_searched_as_text() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        seed(
            &conn,
            "arxiv:2204.12985",
            "the manifold hypothesis in latent space",
        );

        assert!(search_full_text(&conn, "full_text:manifold", 20)
            .unwrap()
            .is_empty());
        let hits = search_full_text(&conn, "manifold hypothesis", 20).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source_id, "arxiv:2204.12985");
    }

    /// Quoted phrases stay phrases, so word order still narrows a search.
    #[test]
    fn quoted_phrase_requires_adjacency() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        seed(
            &conn,
            "arxiv:2204.12985",
            "latent space and manifold learning",
        );

        assert_eq!(
            search_full_text(&conn, "\"manifold learning\"", 20)
                .unwrap()
                .len(),
            1
        );
        assert!(search_full_text(&conn, "\"manifold space\"", 20)
            .unwrap()
            .is_empty());
        // An unterminated quote runs to end of input instead of raising.
        assert_eq!(
            search_full_text(&conn, "\"manifold learning", 20)
                .unwrap()
                .len(),
            1
        );
    }

    /// The invariant the rewrite rests on: whatever `match_expr` emits parses as
    /// FTS5, for any input at all. An expression FTS5 rejects comes back through
    /// `fts_matches` as "no rows", which is indistinguishable from a genuine
    /// empty search, so a regression here would be invisible from the UI. Runs
    /// MATCH directly and unwraps, going around that swallow.
    #[test]
    fn every_match_expr_output_parses_as_fts5() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();

        // Every character FTS5 gives meaning to, plus the ones that broke it.
        let atoms = [
            "\"",
            "-",
            ":",
            "(",
            ")",
            "*",
            "^",
            "%",
            "+",
            "&",
            "|",
            "{",
            "}",
            "[",
            "]",
            "AND",
            "OR",
            "NOT",
            "NEAR",
            "attention",
            "a",
            "\\",
            "/",
            "~",
            "!",
            ",",
            "日本語",
            "\0",
            "0",
            "'",
        ];
        let mut checked = 0;
        for a in atoms {
            for b in atoms {
                for c in atoms {
                    for raw in [format!("{a}{b}{c}"), format!("{a} {b} {c}")] {
                        let Some(expr) = match_expr(&raw) else {
                            continue;
                        };
                        conn.query_row(
                            "SELECT count(*) FROM papers_fts WHERE papers_fts MATCH ?1",
                            [&expr],
                            |r| r.get::<_, i64>(0),
                        )
                        .unwrap_or_else(|e| panic!("{raw:?} -> {expr:?} rejected by FTS5: {e}"));
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 1000, "only {checked} expressions reached FTS5");
    }

    #[test]
    fn match_expr_quotes_terms_and_keeps_operators() {
        assert_eq!(
            match_expr("encoder-decoder").unwrap(),
            "\"encoder-decoder\""
        );
        assert_eq!(
            match_expr("neural networks").unwrap(),
            "\"neural\" \"networks\""
        );
        assert_eq!(match_expr("a OR b").unwrap(), "\"a\" OR \"b\"");
        assert_eq!(match_expr("dot-product*").unwrap(), "\"dot-product\"*");
        assert_eq!(match_expr("\"exact phrase\"").unwrap(), "\"exact phrase\"");
        // A quote mid-query starts a fresh quoted-phrase term rather than being
        // swallowed into the prior bare word.
        assert_eq!(
            match_expr("say \"hi\" now").unwrap(),
            "\"say\" \"hi\" \"now\""
        );
        assert_eq!(
            match_expr("\"scaled dot-product\"*").unwrap(),
            "\"scaled dot-product\"*"
        );
        assert_eq!(match_expr("\"a b\" *").unwrap(), "\"a b\"");
        // Dangling and doubled operators would each be a syntax error.
        assert_eq!(match_expr("a AND").unwrap(), "\"a\"");
        assert_eq!(match_expr("AND a").unwrap(), "\"a\"");
        assert_eq!(match_expr("a AND OR b").unwrap(), "\"a\" AND \"b\"");
        assert_eq!(match_expr(""), None);
        assert_eq!(match_expr("  -%- "), None);
        assert_eq!(match_expr("foo\0bar").unwrap(), "\"foobar\"");
    }

    #[test]
    fn notes_fts_stays_in_sync_on_update_and_delete() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        seed(&conn, "arxiv:2204.12985", "some tex source");

        conn.execute(
            "INSERT INTO NOTE (SOURCE_FK, TITLE, NOTE) \
             SELECT SOURCE_FK, 'n', 'aardvark reminder' \
             FROM PAPER_ROOTS WHERE SOURCE_ID = ?1",
            ["arxiv:2204.12985"],
        )
        .unwrap();
        let note_sk: i64 = conn
            .query_row(
                "SELECT NOTE_SK FROM NOTE WHERE NOTE = 'aardvark reminder'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(search_full_text(&conn, "aardvark", 20).unwrap().len(), 1);

        conn.execute(
            "UPDATE NOTE SET NOTE = 'buffalo reminder' WHERE NOTE_SK = ?1",
            [note_sk],
        )
        .unwrap();
        assert_eq!(search_full_text(&conn, "aardvark", 20).unwrap().len(), 0);
        assert_eq!(search_full_text(&conn, "buffalo", 20).unwrap().len(), 1);

        conn.execute("DELETE FROM NOTE WHERE NOTE_SK = ?1", [note_sk])
            .unwrap();
        assert_eq!(search_full_text(&conn, "buffalo", 20).unwrap().len(), 0);
    }
}
